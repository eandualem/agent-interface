//! Read theme configuration without changing files or installing a palette.

use agent_interface_core::theme::{MAX_BYTES, Theme};
use std::ffi::OsString;
use std::fs::OpenOptions;
use std::io::{self, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use uzers::os::unix::UserExt;

/// A read result, including the selected path even when no file exists.
#[derive(Debug)]
pub struct Loaded {
    /// Valid configuration, or the supplied working theme on a read failure.
    pub theme: Theme,
    /// Absolute path selected from the invoking environment, without resolving links.
    pub path: PathBuf,
    /// Reason first, path last; absent for a missing file. Control characters are escaped.
    pub diagnostic: Option<String>,
}

/// Read once. Missing files silently keep `working` (or the shipped default);
/// invalid and unreadable files keep it with a diagnostic. Only path resolution
/// can return an error. This neither activates nor writes a theme.
pub fn load(explicit: Option<&Path>, working: Option<&Theme>) -> io::Result<Loaded> {
    load_with(explicit, working, &|name| std::env::var_os(name))
}

fn load_with(
    explicit: Option<&Path>,
    working: Option<&Theme>,
    env: &impl Fn(&str) -> Option<OsString>,
) -> io::Result<Loaded> {
    let path = source(explicit, env)?;
    let read = (|| {
        // Open nonblocking before checking the handle: following dotfile links
        // must not allow a FIFO, including a raced replacement, to hang startup.
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&path)?;
        if !file.metadata()?.is_file() {
            return Err(io::Error::other("not a regular file"));
        }
        let mut bytes = Vec::new();
        file.take((MAX_BYTES + 1) as u64).read_to_end(&mut bytes)?;
        Ok(bytes)
    })();
    let fallback = || working.cloned().unwrap_or_default();
    let shown_path = path
        .as_os_str()
        .to_string_lossy()
        .escape_debug()
        .to_string();
    let (theme, diagnostic) = match read {
        Ok(bytes) => match Theme::parse(&bytes) {
            Ok(theme) => (theme, None),
            Err(error) => (
                fallback(),
                Some(format!("{error}; using current colors ({shown_path})")),
            ),
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => (fallback(), None),
        Err(error) => (
            fallback(),
            Some(format!(
                "{}: {shown_path}",
                error.to_string().escape_debug()
            )),
        ),
    };
    Ok(Loaded {
        theme,
        path,
        diagnostic,
    })
}

fn source(explicit: Option<&Path>, env: &impl Fn(&str) -> Option<OsString>) -> io::Result<PathBuf> {
    let configured = env("AGENT_INTERFACE_THEME")
        .filter(|p| !p.is_empty())
        .or_else(|| env("TMUX_WORKSPACES_THEME").filter(|p| !p.is_empty()));
    let home = || {
        env("HOME")
            // Python's POSIX expanduser("~") returns / for an empty HOME.
            .map(|path| {
                if path.is_empty() {
                    PathBuf::from("/")
                } else {
                    PathBuf::from(path)
                }
            })
            .or_else(|| {
                uzers::get_user_by_uid(uzers::get_current_uid())
                    .map(|user| user.home_dir().to_owned())
            })
            .ok_or_else(|| io::Error::other("cannot resolve home directory for theme"))
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
        selected = Some(crate::compat::config_file(&config, "theme.toml"));
    }
    let mut path = selected.unwrap();
    if let Ok(suffix) = path.strip_prefix("~") {
        path = home()?.join(suffix);
    }
    if !path.is_absolute() {
        path = std::env::current_dir()?.join(path);
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap, fs};

    #[test]
    fn loading_obeys_paths_and_preserves_working_colors_on_failure() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path();
        let config = root.join("config/agent-interface");
        fs::create_dir_all(&config).unwrap();
        fs::write(config.join("theme.toml"), "preset = 'paper'").unwrap();
        let chosen = root.join("chosen.toml");
        fs::write(&chosen, "preset = 'plain'").unwrap();
        let mut env = HashMap::from([
            ("HOME", root.as_os_str().to_owned()),
            ("XDG_CONFIG_HOME", root.join("config").into_os_string()),
        ]);
        assert_eq!(
            load_with(None, None, &|k| env.get(k).cloned())
                .unwrap()
                .theme
                .base_preset(),
            "paper"
        );
        env.insert("AGENT_INTERFACE_THEME", chosen.as_os_str().to_owned());
        assert_eq!(
            load_with(None, None, &|k| env.get(k).cloned())
                .unwrap()
                .theme
                .base_preset(),
            "plain"
        );
        let working = load_with(Some(&config.join("theme.toml")), None, &|k| {
            env.get(k).cloned()
        })
        .unwrap()
        .theme;
        assert_eq!(working.base_preset(), "paper");
        let alias = root.join("alias.toml");
        std::os::unix::fs::symlink(&chosen, &alias).unwrap();
        let loaded = load_with(Some(&alias), None, &|k| env.get(k).cloned()).unwrap();
        assert_eq!(loaded.path, alias);
        assert_eq!(loaded.theme.base_preset(), "plain");
        assert_eq!(
            source(Some(Path::new("~/chosen.toml")), &|k| env.get(k).cloned()).unwrap(),
            chosen
        );
        assert_eq!(fs::read_to_string(&chosen).unwrap(), "preset = 'plain'");
        fs::remove_file(&chosen).unwrap();
        let missing = load_with(Some(&chosen), Some(&working), &|k| env.get(k).cloned()).unwrap();
        assert_eq!(missing.theme, working);
        assert!(missing.diagnostic.is_none());
        assert!(!chosen.exists());
        for bytes in [
            b"\xff".to_vec(),
            vec![b' '; MAX_BYTES + 1],
            b"[normal".to_vec(),
        ] {
            fs::write(&chosen, &bytes).unwrap();
            let loaded =
                load_with(Some(&chosen), Some(&working), &|k| env.get(k).cloned()).unwrap();
            assert_eq!(loaded.theme, working);
            assert!(
                loaded
                    .diagnostic
                    .unwrap()
                    .ends_with(&format!("({})", chosen.display()))
            );
            assert_eq!(fs::read(&chosen).unwrap(), bytes);
        }
        let directory = load_with(Some(root), None, &|k| env.get(k).cloned()).unwrap();
        assert_eq!(directory.theme, Theme::default());
        assert!(directory.diagnostic.is_some());
        let fifo = root.join("fifo");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        let loaded = load_with(Some(&fifo), Some(&working), &|k| env.get(k).cloned()).unwrap();
        assert_eq!(loaded.theme, working);
        assert!(
            loaded
                .diagnostic
                .unwrap()
                .starts_with("not a regular file:")
        );
        env.insert("HOME", OsString::new());
        assert_eq!(
            source(Some(Path::new("~/theme.toml")), &|k| env.get(k).cloned()).unwrap(),
            PathBuf::from("/theme.toml")
        );
    }
}
