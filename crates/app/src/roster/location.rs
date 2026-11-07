use std::{
    ffi::{OsStr, OsString},
    io,
    path::PathBuf,
};

use crate::library::location::{expand_home, resolve_path, user_home};

/// Resolve the optional provider's directory.
///
/// An unrequested provider is used only when its database exists; `requested`
/// returns the directory either way. The directory is nonempty
/// `BACKBONE_DATA_DIR`, else `~/.local/share/agent-backbone`. Resolve home,
/// relative paths and symlinks without creating files. This reads no
/// credentials or database contents and starts no worker; call it before
/// `RosterSource::start`.
pub fn provider_directory(requested: bool) -> io::Result<Option<PathBuf>> {
    resolve(requested, &|name| std::env::var_os(name), &user_home)
}

fn resolve(
    requested: bool,
    env: &impl Fn(&str) -> Option<OsString>,
    home: &impl Fn(Option<&OsStr>) -> Option<PathBuf>,
) -> io::Result<Option<PathBuf>> {
    let selected = env("BACKBONE_DATA_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| "~/.local/share/agent-backbone".into());
    let expanded = expand_home(&selected, env, home).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "cannot resolve the provider home directory",
        )
    })?;
    let directory = resolve_path(&expanded)?;
    if !requested && !directory.join("backbone.db").exists() {
        return Ok(None);
    }
    Ok(Some(directory))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap, fs};

    #[test]
    fn enablement_and_directory_precedence_match_launch_without_creating_files() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().canonicalize().unwrap();
        let mut values = HashMap::<&str, OsString>::new();
        values.insert("HOME", root.as_os_str().to_owned());
        let pick = |requested, values: &HashMap<&str, OsString>| {
            resolve(
                requested,
                &|name| {
                    assert!(matches!(name, "BACKBONE_DATA_DIR" | "HOME"));
                    values.get(name).cloned()
                },
                &|_| None,
            )
        };
        let default = root.join(".local/share/agent-backbone");
        assert!(pick(false, &values).unwrap().is_none());
        assert_eq!(pick(true, &values).unwrap(), Some(default));
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);

        values.insert("BACKBONE_DATA_DIR", root.as_os_str().to_owned());
        assert!(pick(false, &values).unwrap().is_none());
        // Existence enables the adapter; malformed content is handled lazily.
        fs::write(root.join("backbone.db"), "invalid fixture database").unwrap();
        fs::write(root.join(".env"), "BACKBONE_API_KEY='invalid fixture quote").unwrap();
        assert_eq!(pick(false, &values).unwrap(), Some(root.clone()));
        std::os::unix::fs::symlink(&root, root.join("alias")).unwrap();
        values.insert("BACKBONE_DATA_DIR", root.join("alias").into_os_string());
        assert_eq!(pick(false, &values).unwrap(), Some(root.clone()));
        values.insert("BACKBONE_DATA_DIR", "relative-provider".into());
        assert_eq!(
            pick(true, &values).unwrap(),
            Some(std::env::current_dir().unwrap().join("relative-provider"))
        );
        values.insert("BACKBONE_DATA_DIR", "".into());
        assert_eq!(
            pick(true, &values).unwrap(),
            Some(root.join(".local/share/agent-backbone"))
        );
        assert_eq!(fs::read_dir(&root).unwrap().count(), 3);
        assert_eq!(
            fs::read_to_string(root.join("backbone.db")).unwrap(),
            "invalid fixture database"
        );
        assert_eq!(
            fs::read_to_string(root.join(".env")).unwrap(),
            "BACKBONE_API_KEY='invalid fixture quote"
        );
    }
}
