//! Read one keymap without creating a library, terminal, directory or config file.

use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::{self, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use tmux_workspaces_core::keymap::{Keymap, MAX_BYTES};
use uzers::os::unix::UserExt;

/// Read the effective map. `disabled` skips all file reads; it cannot be combined
/// with an explicit path. Only a missing implicit default uses shipped bindings.
/// Paths use the invoking environment and directory, without symlink resolution.
pub fn load(explicit: Option<&Path>, disabled: bool) -> io::Result<Keymap> {
    load_with(explicit, disabled, &|name| std::env::var_os(name))
}

fn load_with(
    explicit: Option<&Path>,
    disabled: bool,
    env: &impl Fn(&str) -> Option<OsString>,
) -> io::Result<Keymap> {
    if disabled {
        return if explicit.is_some() {
            Err(io::Error::other(
                "an explicit keymap path cannot be combined with disabled keymaps",
            ))
        } else {
            Ok(Keymap::default())
        };
    }
    let (path, required) = source(explicit, env)?;
    let result = (|| {
        // Follow dotfile symlinks, but open nonblocking before inspecting the
        // handle so a FIFO (including a raced replacement) cannot block startup.
        let file = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&path)
        {
            Err(error) if !required && error.kind() == io::ErrorKind::NotFound => {
                return Ok(Keymap::default());
            }
            result => result?,
        };
        if !file.metadata()?.is_file() {
            return Err(io::Error::other("not a regular file"));
        }
        let mut bytes = Vec::new();
        file.take((MAX_BYTES + 1) as u64).read_to_end(&mut bytes)?;
        Keymap::parse(&bytes).map_err(io::Error::other)
    })();
    result.map_err(|error| {
        io::Error::new(error.kind(), format!("keymap {}: {error}", path.display()))
    })
}

fn source(
    explicit: Option<&Path>,
    env: &impl Fn(&str) -> Option<OsString>,
) -> io::Result<(PathBuf, bool)> {
    let configured = env("TMUX_WORKSPACES_KEYMAP").filter(|p| !p.is_empty());
    let required = explicit.is_some() || configured.is_some();
    let home = || {
        env("HOME")
            .map(PathBuf::from)
            .or_else(|| {
                uzers::get_user_by_uid(uzers::get_current_uid())
                    .map(|user| user.home_dir().to_owned())
            })
            .ok_or_else(|| io::Error::other("cannot resolve home directory for keymap"))
    };
    let mut selected = explicit
        .map(|path| {
            if path.as_os_str().is_empty() {
                PathBuf::from(".")
            } else {
                path.to_owned()
            }
        })
        .or_else(|| configured.map(PathBuf::from));
    if selected.is_none() {
        let config = match env("XDG_CONFIG_HOME").filter(|p| !p.is_empty()) {
            Some(path) => PathBuf::from(path),
            None => home()?.join(".config"),
        };
        selected = Some(config.join("tmux-workspaces/keymap.toml"));
    }
    let mut path = selected.unwrap();
    if let Ok(suffix) = path.strip_prefix("~") {
        path = home()?.join(suffix);
    }
    if !path.is_absolute() {
        path = std::env::current_dir()?.join(path);
    }
    Ok((path, required))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::fs;

    #[test]
    fn loading_obeys_precedence_limits_and_keeps_files_unchanged() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path();
        let config = root.join("config/tmux-workspaces");
        fs::create_dir_all(&config).unwrap();
        fs::write(config.join("keymap.toml"), "prefix = 'C-a'").unwrap();
        let chosen = root.join("chosen.toml");
        fs::write(&chosen, "prefix = 'C-b'").unwrap();
        let mut env = HashMap::from([
            ("HOME", root.as_os_str().to_owned()),
            ("XDG_CONFIG_HOME", root.join("config").into_os_string()),
        ]);
        assert_eq!(
            load_with(None, false, &|k| env.get(k).cloned())
                .unwrap()
                .prefix(),
            "C-a"
        );
        env.insert("TMUX_WORKSPACES_KEYMAP", chosen.as_os_str().to_owned());
        assert_eq!(
            load_with(None, false, &|k| env.get(k).cloned())
                .unwrap()
                .prefix(),
            "C-b"
        );
        assert_eq!(
            load_with(Some(&config.join("keymap.toml")), false, &|k| env
                .get(k)
                .cloned())
            .unwrap()
            .prefix(),
            "C-a"
        );
        assert_eq!(
            load_with(Some(Path::new("~/chosen.toml")), false, &|k| env
                .get(k)
                .cloned())
            .unwrap()
            .prefix(),
            "C-b"
        );
        assert_eq!(
            load_with(None, true, &|_| panic!(
                "disabled must not read environment"
            ))
            .unwrap(),
            Keymap::default()
        );
        assert!(load_with(Some(&chosen), true, &|_| None).is_err());
        assert_eq!(fs::read_to_string(&chosen).unwrap(), "prefix = 'C-b'");
        fs::remove_file(&chosen).unwrap();
        assert!(load_with(None, false, &|k| env.get(k).cloned()).is_err());
        env.remove("TMUX_WORKSPACES_KEYMAP");
        fs::remove_file(config.join("keymap.toml")).unwrap();
        assert_eq!(
            load_with(None, false, &|k| env.get(k).cloned()).unwrap(),
            Keymap::default()
        );
        for bytes in [
            b"\xff".to_vec(),
            vec![b'#'; MAX_BYTES + 1],
            b"[bindings".to_vec(),
        ] {
            fs::write(&chosen, &bytes).unwrap();
            assert!(
                load_with(Some(&chosen), false, &|k| env.get(k).cloned())
                    .unwrap_err()
                    .to_string()
                    .contains("chosen.toml")
            );
            assert_eq!(fs::read(&chosen).unwrap(), bytes);
        }
        assert!(load_with(Some(root), false, &|k| env.get(k).cloned()).is_err());
        fs::write(&chosen, "prefix = 'C-e'").unwrap();
        let alias = root.join("alias.toml");
        std::os::unix::fs::symlink(&chosen, &alias).unwrap();
        assert_eq!(
            load_with(Some(&alias), false, &|k| env.get(k).cloned())
                .unwrap()
                .prefix(),
            "C-e"
        );
    }
}
