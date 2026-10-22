use std::{
    ffi::{OsStr, OsString},
    io,
    path::{Path, PathBuf},
};

use crate::library::location::{expand_home, resolve_path, user_home};

/// Decide whether the optional provider is enabled and resolve its directory.
///
/// Follow the Python launch rules: explicit disable wins, demo uses its own
/// roster, and an unrequested provider runs only when its database exists.
/// Directory precedence is the explicit path, nonempty `BACKBONE_DATA_DIR`, then
/// `~/.local/share/agent-backbone`. Resolve home, relative paths and symlinks
/// without creating files. This reads no credentials or database contents and
/// starts no worker; call it before `RosterSource::start`.
pub fn provider_directory(
    enabled: Option<bool>,
    explicit: Option<&Path>,
    url: Option<&str>,
    demo: bool,
) -> io::Result<Option<PathBuf>> {
    resolve(
        enabled,
        explicit,
        url,
        demo,
        &|name| std::env::var_os(name),
        &user_home,
    )
}

fn resolve(
    enabled: Option<bool>,
    explicit: Option<&Path>,
    url: Option<&str>,
    demo: bool,
    env: &impl Fn(&str) -> Option<OsString>,
    home: &impl Fn(Option<&OsStr>) -> Option<PathBuf>,
) -> io::Result<Option<PathBuf>> {
    if enabled == Some(false) {
        if explicit.is_some() || url.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "--no-backbone contradicts --backbone-data-dir and --url",
            ));
        }
        return Ok(None);
    }
    if demo {
        if enabled == Some(true) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "--demo and --backbone are exclusive",
            ));
        }
        return Ok(None);
    }
    let selected = explicit.map(Path::to_owned).unwrap_or_else(|| {
        env("BACKBONE_DATA_DIR")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| "~/.local/share/agent-backbone".into())
    });
    let expanded = expand_home(&selected, env, home).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "cannot resolve the provider home directory",
        )
    })?;
    let directory = resolve_path(&expanded)?;
    let unasked = enabled.is_none() && explicit.is_none() && url.is_none();
    if unasked && !directory.join("backbone.db").exists() {
        return Ok(None);
    }
    Ok(Some(directory))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap, fs};

    #[test]
    fn disabled_and_demo_do_not_resolve_or_read_provider_configuration() {
        let env = |_: &str| -> Option<OsString> { panic!("provider environment was read") };
        let home = |_: Option<&OsStr>| -> Option<PathBuf> { panic!("home was resolved") };
        assert!(
            resolve(Some(false), None, None, false, &env, &home)
                .unwrap()
                .is_none()
        );
        assert!(
            resolve(
                None,
                Some(Path::new("missing")),
                Some("bad url"),
                true,
                &env,
                &home
            )
            .unwrap()
            .is_none()
        );
        assert!(
            resolve(Some(false), None, None, true, &env, &home)
                .unwrap()
                .is_none()
        );
        assert!(resolve(Some(false), Some(Path::new("")), None, false, &env, &home).is_err());
        assert!(resolve(Some(false), None, Some(""), true, &env, &home).is_err());
        assert!(resolve(Some(true), None, None, true, &env, &home).is_err());
        assert!(
            provider_directory(Some(false), None, None, false)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn enablement_and_directory_precedence_match_launch_without_creating_files() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().canonicalize().unwrap();
        let mut values = HashMap::<&str, OsString>::new();
        values.insert("HOME", root.as_os_str().to_owned());
        let pick = |enabled, explicit: Option<&Path>, url, values: &HashMap<&str, OsString>| {
            resolve(
                enabled,
                explicit,
                url,
                false,
                &|name| {
                    assert!(matches!(name, "BACKBONE_DATA_DIR" | "HOME"));
                    values.get(name).cloned()
                },
                &|_| None,
            )
        };
        let default = root.join(".local/share/agent-backbone");
        assert!(pick(None, None, None, &values).unwrap().is_none());
        assert_eq!(
            pick(Some(true), None, None, &values).unwrap(),
            Some(default.clone())
        );
        assert_eq!(pick(None, None, Some(""), &values).unwrap(), Some(default));
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);

        values.insert("BACKBONE_DATA_DIR", root.as_os_str().to_owned());
        assert!(pick(None, None, None, &values).unwrap().is_none());
        // Existence enables the adapter; malformed content is handled lazily.
        fs::write(root.join("backbone.db"), "invalid fixture database").unwrap();
        fs::write(root.join(".env"), "BACKBONE_API_KEY='invalid fixture quote").unwrap();
        assert_eq!(pick(None, None, None, &values).unwrap(), Some(root.clone()));
        assert_eq!(
            pick(None, Some(Path::new("~/missing")), None, &values).unwrap(),
            Some(root.join("missing"))
        );
        std::os::unix::fs::symlink(&root, root.join("alias")).unwrap();
        assert_eq!(
            pick(None, Some(&root.join("alias/not-created")), None, &values).unwrap(),
            Some(root.join("not-created"))
        );
        assert_eq!(
            pick(None, Some(Path::new("")), None, &values).unwrap(),
            Some(std::env::current_dir().unwrap())
        );
        values.insert("BACKBONE_DATA_DIR", "relative-provider".into());
        assert_eq!(
            pick(Some(true), None, None, &values).unwrap(),
            Some(std::env::current_dir().unwrap().join("relative-provider"))
        );
        values.insert("BACKBONE_DATA_DIR", "".into());
        assert_eq!(
            pick(Some(true), None, None, &values).unwrap(),
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
