//! Read pre-rename user data in place. New writes use the current identity unless
//! moving the library would change its tmux socket hash.

use std::path::{Path, PathBuf};

pub(crate) fn existing_or_legacy(preferred: PathBuf, legacy: PathBuf) -> PathBuf {
    if matches!(preferred.symlink_metadata(), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
        && legacy.symlink_metadata().is_ok()
    {
        legacy
    } else {
        preferred
    }
}

pub(crate) fn config_file(config: &Path, name: &str) -> PathBuf {
    existing_or_legacy(
        config.join("agent-interface").join(name),
        config.join("tmux-workspaces").join(name),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_is_used_only_until_a_new_file_exists() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("tmux-workspaces");
        let new = dir.path().join("agent-interface");
        std::fs::create_dir(&old).unwrap();
        std::fs::write(old.join("settings.toml"), "old").unwrap();
        assert_eq!(
            config_file(dir.path(), "settings.toml"),
            old.join("settings.toml")
        );
        std::fs::create_dir(&new).unwrap();
        std::fs::write(new.join("settings.toml"), "new").unwrap();
        assert_eq!(
            config_file(dir.path(), "settings.toml"),
            new.join("settings.toml")
        );
        assert_eq!(
            std::fs::read_to_string(old.join("settings.toml")).unwrap(),
            "old"
        );
    }
}
