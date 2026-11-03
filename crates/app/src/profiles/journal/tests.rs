use super::*;
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    sync::{Arc, Barrier},
    thread,
};
const ID: &str = "11111111111111111111111111111111";
const OTHER: &str = "22222222222222222222222222222222";
fn fixture() -> (tempfile::TempDir, std::path::PathBuf, Journal) {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().canonicalize().unwrap().join("journal");
    let journal = Journal::open(&path).unwrap();
    (root, path, journal)
}
#[test]
fn durable_records_restore_exact_capsules_and_reject_stale_revisions() {
    use crate::profiles::{Client, lifecycle::Operation};
    use agent_interface_roster::profiles::operations::Prepared;
    let (_root, path, journal) = fixture();
    let client = Client::new(path.join("provider"), "/usr/bin:/bin".into()).unwrap();
    let operation = Operation::new(&client, Prepared::create(ID, "Work", "baseline").unwrap());
    let capsule = operation.checkpoint().unwrap();
    let first = journal.save(ID, None, &capsule).unwrap();
    assert_eq!(first.revision(), 1);
    assert_eq!(first.id(), ID);
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o700);
    assert_eq!(fs::metadata(path.join(ID)).unwrap().mode() & 0o777, 0o600);
    drop(journal);
    let journal = Journal::open(&path).unwrap();
    let saved = journal.read(ID).unwrap().unwrap();
    let recovered = Operation::restore(&client, saved.payload()).unwrap();
    assert_eq!(recovered.request_bytes(), operation.request_bytes());
    assert_eq!(journal.ids().unwrap(), [ID]);
    assert!(matches!(
        journal.save(ID, None, b"replacement"),
        Err(Error::Conflict)
    ));
    assert_eq!(
        journal
            .save(ID, Some(saved.revision()), b"new observation")
            .unwrap()
            .revision(),
        2
    );
    assert!(matches!(
        journal.save(ID, Some(saved.revision()), b"stale"),
        Err(Error::Conflict)
    ));
    assert_eq!(
        journal.read(ID).unwrap().unwrap().payload(),
        b"new observation"
    );
    assert!(journal.read(OTHER).unwrap().is_none());
    assert_eq!(fs::read_dir(&path).unwrap().count(), 2);
}
#[test]
fn independent_handles_cannot_overwrite_a_concurrent_winner() {
    // Parallel process-spawning tests may briefly inherit a CLOEXEC lock
    // descriptor before exec. Busy is an expected API result, not corruption.
    fn eventually<T>(mut operation: impl FnMut() -> Result<T, Error>) -> Result<T, Error> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match operation() {
                Err(Error::Busy) if std::time::Instant::now() < deadline => {
                    thread::sleep(std::time::Duration::from_millis(5))
                }
                result => return result,
            }
        }
    }
    let (_root, path, journal) = fixture();
    journal.save(ID, None, b"original").unwrap();
    let lock = eventually(|| journal.lock()).unwrap();
    assert!(matches!(
        Journal::open(&path).unwrap().save(ID, Some(1), b"blocked"),
        Err(Error::Busy)
    ));
    drop(lock);
    let barrier = Arc::new(Barrier::new(2));
    let workers: Vec<_> = [b"left".to_vec(), b"right".to_vec()]
        .into_iter()
        .map(|payload| {
            let path = path.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                let journal = Journal::open(&path).unwrap();
                barrier.wait();
                eventually(|| journal.save(ID, Some(1), &payload)).map(|r| r.revision())
            })
        })
        .collect();
    let results: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| **r == Ok(2)).count(), 1);
    assert!(
        results
            .iter()
            .any(|r| matches!(r, Err(Error::Busy | Error::Conflict)))
    );
    let stored = eventually(|| journal.read(ID)).unwrap().unwrap();
    assert_eq!(stored.revision(), 2);
    assert!(stored.payload() == b"left" || stored.payload() == b"right");
}
#[test]
fn child_cannot_take_an_existing_journal_lock() {
    let Some(path) = std::env::var_os("JOURNAL_LOCK_FIXTURE") else {
        return;
    };
    let journal = Journal::open(Path::new(&path)).unwrap();
    assert!(matches!(journal.save(ID, None, b"child"), Err(Error::Busy)));
}
#[test]
fn journal_lock_excludes_another_process() {
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    let (_root, path, journal) = fixture();
    let _lock = journal.lock().unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "profiles::journal::tests::child_cannot_take_an_existing_journal_lock",
        ])
        .env("JOURNAL_LOCK_FIXTURE", &path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("fixture lock wait exceeded deadline");
        }
        thread::sleep(Duration::from_millis(10));
    }
}
#[test]
fn links_and_nonprivate_files_never_redirect_or_expose_records() {
    let (root, path, journal) = fixture();
    let external = root.path().join("external");
    fs::write(&external, b"untouched").unwrap();
    fs::set_permissions(&external, fs::Permissions::from_mode(0o600)).unwrap();
    symlink(&external, path.join(ID)).unwrap();
    assert!(matches!(journal.read(ID), Err(Error::Unsafe)));
    assert!(matches!(
        journal.save(ID, None, b"overwrite"),
        Err(Error::Unsafe)
    ));
    assert_eq!(fs::read(&external).unwrap(), b"untouched");
    fs::remove_file(path.join(ID)).unwrap();
    fs::hard_link(&external, path.join(ID)).unwrap();
    assert!(matches!(journal.read(ID), Err(Error::Unsafe)));
    fs::remove_file(path.join(ID)).unwrap();
    journal.save(ID, None, b"private").unwrap();
    fs::set_permissions(path.join(ID), fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(journal.read(ID), Err(Error::Unsafe)));
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(journal.ids(), Err(Error::Unsafe)));
    let alias = root.path().join("alias");
    symlink(&path, &alias).unwrap();
    assert!(matches!(Journal::open(&alias), Err(Error::Unsafe)));
    assert!(matches!(
        Journal::open(&alias.join("nested")),
        Err(Error::Unsafe)
    ));
}
#[test]
fn directory_handles_stay_pinned_after_path_replacement() {
    let (root, path, journal) = fixture();
    let moved = root.path().join("moved");
    fs::rename(&path, &moved).unwrap();
    let replacement = Journal::open(&path).unwrap();
    journal.save(ID, None, b"original directory").unwrap();
    assert!(replacement.read(ID).unwrap().is_none());
    assert_eq!(
        Journal::open(&moved.canonicalize().unwrap())
            .unwrap()
            .read(ID)
            .unwrap()
            .unwrap()
            .payload(),
        b"original directory"
    );
}
#[test]
fn corruption_limits_and_invalid_ids_preserve_existing_records() {
    let (_root, path, journal) = fixture();
    journal.save(ID, None, b"original").unwrap();
    let original = fs::read(path.join(ID)).unwrap();
    let mut corrupt = original.clone();
    *corrupt.last_mut().unwrap() ^= 1;
    fs::write(path.join(ID), corrupt).unwrap();
    assert!(matches!(journal.read(ID), Err(Error::Invalid)));
    assert!(matches!(
        journal.save(ID, Some(1), b"replacement"),
        Err(Error::Invalid)
    ));
    fs::write(path.join(ID), &original[..HEADER - 1]).unwrap();
    assert!(matches!(journal.read(ID), Err(Error::Invalid)));
    fs::write(path.join(ID), &original).unwrap();
    fs::write(path.join(OTHER), &original).unwrap();
    fs::set_permissions(path.join(OTHER), fs::Permissions::from_mode(0o600)).unwrap();
    assert!(matches!(journal.read(OTHER), Err(Error::Invalid)));
    for id in ["../escape", "", "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"] {
        assert!(matches!(journal.save(id, None, b"x"), Err(Error::Invalid)));
    }
    assert!(matches!(
        journal.save(ID, Some(1), &vec![b'x'; LIMIT + 1]),
        Err(Error::TooLarge)
    ));
    assert_eq!(journal.read(ID).unwrap().unwrap().payload(), b"original");
    let max = vec![b'x'; LIMIT];
    journal.save(ID, Some(1), &max).unwrap();
    assert_eq!(journal.read(ID).unwrap().unwrap().payload(), max);
}

#[test]
fn full_directory_refuses_new_records_before_replacement() {
    use std::os::unix::fs::OpenOptionsExt;
    let (_root, path, journal) = fixture();
    for number in 0..MAX_RECORDS {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path.join(format!("{number:032x}")))
            .unwrap();
    }
    assert_eq!(journal.ids().unwrap().len(), MAX_RECORDS);
    assert!(matches!(
        journal.save(ID, None, b"new"),
        Err(Error::TooLarge)
    ));
    assert!(!path.join(ID).exists());
}

#[test]
fn interrupted_initialization_requires_parent_sync_even_when_directory_exists() {
    use std::cell::Cell;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().canonicalize().unwrap().join("journal");
    assert!(matches!(
        Journal::open_with_sync(&path, &|_| Err(Error::Unavailable)),
        Err(Error::Unavailable)
    ));
    assert!(path.is_dir());
    let synced = Cell::new(0);
    let journal = Journal::open_with_sync(&path, &|parent| {
        synced.set(synced.get() + 1);
        parent.sync_all().map_err(|_| Error::Unavailable)
    })
    .unwrap();
    assert_eq!(synced.get(), 1);
    journal.save(ID, None, b"durable").unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn extended_and_inherited_acls_are_refused_despite_private_mode_bits() {
    use std::process::Command;
    fn grant(path: &std::path::Path, rights: &str) {
        assert!(
            Command::new("/bin/chmod")
                .args(["+a", rights])
                .arg(path)
                .status()
                .unwrap()
                .success()
        );
    }
    fn clear(path: &std::path::Path) {
        assert!(
            Command::new("/bin/chmod")
                .arg("-N")
                .arg(path)
                .status()
                .unwrap()
                .success()
        );
    }
    let (root, path, journal) = fixture();
    grant(&path, "everyone allow read,write,execute");
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o700);
    let refused = journal.save(ID, None, b"private");
    clear(&path);
    assert!(matches!(refused, Err(Error::Unsafe)));
    assert!(!path.join(ID).exists());
    journal.save(ID, None, b"private").unwrap();
    grant(&path.join(ID), "everyone allow read,write");
    assert_eq!(fs::metadata(path.join(ID)).unwrap().mode() & 0o777, 0o600);
    let refused = journal.read(ID);
    clear(&path.join(ID));
    assert!(matches!(refused, Err(Error::Unsafe)));
    grant(&path.join(".lock"), "everyone allow read,write");
    let refused = journal.read(ID);
    clear(&path.join(".lock"));
    assert!(matches!(refused, Err(Error::Unsafe)));
    let parent = root.path().canonicalize().unwrap();
    grant(
        &parent,
        "everyone allow read,write,execute,file_inherit,directory_inherit",
    );
    let inherited = parent.join("inherited");
    let refused = Journal::open(&inherited);
    clear(&inherited);
    clear(&parent);
    assert!(matches!(refused, Err(Error::Unsafe)));
    assert_eq!(fs::read_dir(inherited).unwrap().count(), 0);
}

#[test]
fn post_rename_sync_failure_requires_a_fresh_durability_barrier() {
    use std::cell::Cell;
    let (_root, _path, journal) = fixture();
    let first = journal.save(ID, None, b"submitted").unwrap();
    let result = journal.save_with_sync(ID, Some(first.revision()), b"terminal", &|_| {
        Err(Error::Unavailable)
    });
    assert!(matches!(result, Err(Error::Unavailable)));
    // Rename succeeded: visibility is explicitly weaker than durability.
    assert_eq!(journal.read(ID).unwrap().unwrap().payload(), b"terminal");
    assert!(matches!(
        journal.read_durable_with(ID, &|_| Err(Error::Unavailable)),
        Err(Error::Unavailable)
    ));
    let barriers = Cell::new(0);
    let durable = journal
        .read_durable_with(ID, &|directory| {
            barriers.set(barriers.get() + 1);
            directory.sync_all().map_err(|_| Error::Unavailable)
        })
        .unwrap()
        .unwrap();
    assert_eq!(barriers.get(), 1);
    assert_eq!(durable.payload(), b"terminal");
    assert_eq!(durable.revision(), first.revision() + 1);
}
