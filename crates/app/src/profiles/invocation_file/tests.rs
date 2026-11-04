use super::*;
use std::{fs, os::unix::fs::PermissionsExt};

#[test]
fn request_paths_are_private_and_anonymous_sinks_survive_cleanup() {
    let parent = tempfile::tempdir().unwrap();
    let input = InvocationFile::in_directory(parent.path(), b"exact private request").unwrap();
    let path = input.path().to_owned();
    let directory = path.parent().unwrap().to_owned();
    assert_eq!(fs::metadata(&directory).unwrap().mode() & 0o777, 0o700);
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
    assert_eq!(fs::read(&path).unwrap(), b"exact private request");
    drop(input);
    assert!(!directory.exists());

    let output = InvocationFile::in_directory(parent.path(), &[]).unwrap();
    let path = output.path().to_owned();
    let mut file = output.into_file();
    assert!(!path.exists());
    assert_eq!(fs::read_dir(parent.path()).unwrap().count(), 0);
    file.write_all(b"late service output").unwrap();
    assert_eq!(file.metadata().unwrap().len(), 19);
}

#[test]
fn writable_nonsticky_ancestors_are_rejected_before_creating_a_request() {
    let parent = tempfile::tempdir().unwrap();
    let child = parent.path().join("private");
    fs::create_dir(&child).unwrap();
    fs::set_permissions(&child, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(parent.path(), fs::Permissions::from_mode(0o777)).unwrap();
    let result = InvocationFile::in_directory(&child, b"must not be written");
    fs::set_permissions(parent.path(), fs::Permissions::from_mode(0o700)).unwrap();
    assert!(matches!(result, Err(Error::Unavailable)));
    assert_eq!(fs::read_dir(&child).unwrap().count(), 0);
}

#[cfg(target_os = "macos")]
#[test]
fn inheritable_acls_are_rejected_before_sensitive_bytes_are_written() {
    use std::process::Command;
    let parent = tempfile::tempdir().unwrap();
    assert!(
        Command::new("/bin/chmod")
            .args([
                "+a",
                "everyone allow read,write,execute,file_inherit,directory_inherit",
            ])
            .arg(parent.path())
            .status()
            .unwrap()
            .success()
    );
    let result = InvocationFile::in_directory(parent.path(), b"private adoption token");
    assert!(
        Command::new("/bin/chmod")
            .arg("-N")
            .arg(parent.path())
            .status()
            .unwrap()
            .success()
    );
    assert!(matches!(result, Err(Error::Unavailable)));
    assert_eq!(fs::read_dir(parent.path()).unwrap().count(), 0);
}
