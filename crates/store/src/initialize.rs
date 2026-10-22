//! Publish a complete first-run library without replacing an existing file.

use std::{fs, io, os::unix::fs::DirBuilderExt, path::Path};

use rusqlite::{Connection, OpenFlags};
use tmux_workspaces_core::Layout;

use crate::{Document, Error, Store};

impl Store {
    /// Open an existing library read-only, or create an absent library with one
    /// empty workspace. Existing files (including invalid databases and dangling
    /// symlinks) are never initialized or replaced. A concurrent creator wins
    /// without losing its data. No tmux or provider process is started.
    pub fn open_or_create(directory: &Path) -> Result<Self, Error> {
        let path = directory.join("layouts.db");
        let fail = |error: &dyn std::fmt::Display| Error {
            path: path.clone(),
            detail: error.to_string(),
        };
        match fs::symlink_metadata(&path) {
            Ok(_) => return Self::open(directory),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(fail(&error)),
        }
        // A missing main file with recovery data is not a fresh library.
        for suffix in ["-journal", "-wal", "-shm"] {
            match fs::symlink_metadata(directory.join(format!("layouts.db{suffix}"))) {
                Ok(_) => return Err(fail(&"database sidecars exist; preserve them for recovery")),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(fail(&error)),
            }
        }
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)
            .map_err(|error| fail(&error))?;
        let temporary = tempfile::NamedTempFile::new_in(directory).map_err(|error| fail(&error))?;
        let mut model = Layout {
            workspaces: vec![],
            selected: String::new(),
            focus: false,
            show_agents: true,
        };
        model.add_workspace("Workspace 1".into());
        let raw = serde_json::json!({
            "version": 2,
            "workspaces": [{"id": model.selected, "name": "Workspace 1", "tabs": [], "selected": ""}],
            "selected": model.selected,
            "focus": false
        }).to_string();
        let document = Document::parse(raw, &path)?;
        let mut connection =
            Connection::open_with_flags(temporary.path(), OpenFlags::SQLITE_OPEN_READ_WRITE)
                .map_err(|error| fail(&error))?;
        let transaction = connection.transaction().map_err(|error| fail(&error))?;
        transaction
            .execute_batch("CREATE TABLE terminal_layout (id INTEGER PRIMARY KEY, value TEXT)")
            .map_err(|error| fail(&error))?;
        transaction
            .execute(
                "INSERT INTO terminal_layout VALUES (1, ?1)",
                [document.raw_json()],
            )
            .map_err(|error| fail(&error))?;
        transaction.commit().map_err(|error| fail(&error))?;
        connection.close().map_err(|(_, error)| fail(&error))?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|error| fail(&error))?;
        match temporary.persist_noclobber(&path) {
            Ok(_) => {}
            Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(fail(&error.error)),
        }
        Self::open(directory)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        os::unix::fs::{PermissionsExt, symlink},
        sync::{Arc, Barrier},
    };

    #[test]
    fn concurrent_first_launch_creates_one_writable_library_and_preserves_it() {
        let temporary = tempfile::tempdir().unwrap();
        let directory = temporary.path().join("new/library");
        let barrier = Arc::new(Barrier::new(2));
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let directory = directory.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    Store::open_or_create(&directory)
                        .unwrap()
                        .load()
                        .unwrap()
                        .raw_json()
                        .to_owned()
                })
            })
            .collect();
        let documents: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(documents[0], documents[1]);
        let mut store = Store::open_existing_writable(&directory).unwrap();
        let mut base = store.load().unwrap();
        assert!(base.model().workspaces[0].tabs.is_empty());
        let mut model = base.model().clone();
        model.add_tab(Some("Saved tab".into()), "/tmp".into(), true);
        assert!(!store.save(&mut base, &model).unwrap().conflict);
        drop(store);
        let bytes = fs::read(directory.join("layouts.db")).unwrap();
        assert_eq!(
            Store::open_or_create(&directory)
                .unwrap()
                .load()
                .unwrap()
                .model(),
            &model
        );
        assert_eq!(fs::read(directory.join("layouts.db")).unwrap(), bytes);
        assert_eq!(
            fs::metadata(directory.join("layouts.db"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(fs::read_dir(directory).unwrap().count(), 1);
    }

    #[test]
    fn never_repairs_existing_files_or_orphaned_recovery_data() {
        let temporary = tempfile::tempdir().unwrap();
        let directory = temporary.path();
        let path = directory.join("layouts.db");
        for bytes in [b"".as_slice(), b"not a database"] {
            fs::write(&path, bytes).unwrap();
            assert!(
                Store::open_or_create(directory)
                    .and_then(|s| s.load())
                    .is_err()
            );
            assert_eq!(fs::read(&path).unwrap(), bytes);
            fs::remove_file(&path).unwrap();
        }
        symlink("absent", &path).unwrap();
        assert!(Store::open_or_create(directory).is_err());
        assert!(!directory.join("absent").exists());
        assert!(fs::symlink_metadata(&path).unwrap().is_symlink());
        fs::remove_file(&path).unwrap();
        fs::write(directory.join("layouts.db-wal"), b"recovery").unwrap();
        assert!(Store::open_or_create(directory).is_err());
        assert!(!path.exists());
        assert_eq!(
            fs::read(directory.join("layouts.db-wal")).unwrap(),
            b"recovery"
        );
    }
}
