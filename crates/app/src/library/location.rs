use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};
use uzers::os::unix::UserExt;

use tmux_workspaces_store::Error;

/// Find an existing library directory using the Python tool's precedence:
/// explicit path, nonempty `TMUX_WORKSPACES_DATA_DIR`, nonempty `XDG_DATA_HOME`
/// plus `tmux-workspaces`, then `~/.local/share/tmux-workspaces`.
///
/// Expands `~`/`~user` and resolves relative paths and symlinks without creating
/// anything. A missing directory is an error in this read-only phase. Paths are
/// kept as native OS strings, including non-UTF-8 Unix paths.
pub fn library_directory(explicit: Option<&Path>) -> Result<PathBuf, Error> {
    resolve(explicit, &|name| std::env::var_os(name), &user_home)
}

fn user_home(name: Option<&OsStr>) -> Option<PathBuf> {
    let user = match name {
        Some(name) => uzers::get_user_by_name(name),
        None => uzers::get_user_by_uid(uzers::get_current_uid()),
    };
    user.map(|user| user.home_dir().to_owned())
}

/// Resolve the source socket before starting workers or clearing child context.
/// A nonempty explicit path takes precedence, then the invoking `TMUX` socket,
/// then `$TMUX_TMPDIR/tmux-UID/default` (`/tmp` when unset or empty).
///
/// Home, relative paths and symlinks are resolved without creating anything or
/// connecting to a server. The source may not exist yet. Keep this result for
/// the window's lifetime rather than re-reading launcher context on each poll.
pub fn source_socket(explicit: Option<&Path>) -> io::Result<PathBuf> {
    resolve_source(
        explicit,
        &|name| std::env::var_os(name),
        &user_home,
        uzers::get_current_uid(),
    )
}

fn resolve_source(
    explicit: Option<&Path>,
    env: &impl Fn(&str) -> Option<OsString>,
    user_home: &impl Fn(Option<&OsStr>) -> Option<PathBuf>,
    uid: u32,
) -> io::Result<PathBuf> {
    let selected = explicit
        .filter(|path| !path.as_os_str().is_empty())
        .map(Path::to_owned)
        .or_else(|| {
            let identity = env("TMUX")?;
            // Only the two rightmost commas delimit PID and session identity.
            // A socket path itself may contain commas, or native non-UTF-8 bytes.
            let socket = identity.as_bytes().rsplitn(3, |b| *b == b',').nth(2)?;
            (!socket.is_empty()).then(|| PathBuf::from(OsStr::from_bytes(socket)))
        })
        .unwrap_or_else(|| {
            PathBuf::from(
                env("TMUX_TMPDIR")
                    .filter(|v| !v.is_empty())
                    .unwrap_or_else(|| "/tmp".into()),
            )
            .join(format!("tmux-{uid}/default"))
        });
    let expanded = expand_home(&selected, env, user_home).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "cannot resolve the home directory in the source socket path",
        )
    })?;
    resolve_path(&expanded)
}

fn resolve(
    explicit: Option<&Path>,
    env: &impl Fn(&str) -> Option<OsString>,
    user_home: &impl Fn(Option<&OsStr>) -> Option<PathBuf>,
) -> Result<PathBuf, Error> {
    let selected = explicit
        .map(Path::to_owned)
        .or_else(|| {
            env("TMUX_WORKSPACES_DATA_DIR")
                .filter(|p| !p.is_empty())
                .map(PathBuf::from)
        })
        .unwrap_or_else(|| {
            PathBuf::from(
                env("XDG_DATA_HOME")
                    .filter(|p| !p.is_empty())
                    .unwrap_or_else(|| "~/.local/share".into()),
            )
            .join("tmux-workspaces")
        });
    let expanded = expand_home(&selected, env, user_home).ok_or_else(|| Error {
        path: selected.join("layouts.db"),
        detail: "cannot resolve the home directory in the library path".into(),
    })?;
    resolve_path(&expanded)
        .and_then(|path| path.canonicalize())
        .map_err(|error| Error {
            path: expanded.join("layouts.db"),
            detail: error.to_string(),
        })
}

fn resolve_path(path: &Path) -> io::Result<PathBuf> {
    if path.as_os_str().as_bytes().contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "path contains NUL",
        ));
    }
    enum Pending {
        Path(PathBuf),
        SaveLink(PathBuf),
    }
    let mut result = if path.is_absolute() {
        PathBuf::from("/")
    } else {
        std::env::current_dir()?
    };
    let mut pending = vec![Pending::Path(path.to_owned())];
    // None marks an active symlink; Some caches its resolved target. This avoids
    // recursion, repeated expansion and infinite loops, like Python's realpath.
    let mut links: HashMap<PathBuf, Option<PathBuf>> = HashMap::new();
    while let Some(step) = pending.pop() {
        let path = match step {
            Pending::Path(path) => path,
            Pending::SaveLink(link) => {
                links.insert(link, Some(result.clone()));
                continue;
            }
        };
        let mut parts = path.components();
        while let Some(part) = parts.next() {
            match part {
                Component::RootDir => result = PathBuf::from("/"),
                Component::ParentDir => {
                    result.pop();
                }
                Component::Normal(name) => {
                    result.push(name);
                    if let Some(cached) = links.get(&result) {
                        if let Some(target) = cached {
                            result = target.clone();
                        }
                    } else if let Ok(target) = std::fs::read_link(&result) {
                        let link = result.clone();
                        links.insert(link.clone(), None);
                        result.pop();
                        pending.push(Pending::Path(parts.as_path().to_owned()));
                        pending.push(Pending::SaveLink(link));
                        pending.push(Pending::Path(target));
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    // Library lookup canonicalizes this result to require an existing directory;
    // source lookup also accepts a socket that has not been created yet.
    Ok(result)
}

fn expand_home(
    path: &Path,
    env: &impl Fn(&str) -> Option<OsString>,
    user_home: &impl Fn(Option<&OsStr>) -> Option<PathBuf>,
) -> Option<PathBuf> {
    // pathlib removes "." components before expanding the first component.
    let mut components = path.components().filter(|part| *part != Component::CurDir);
    let Some(Component::Normal(first)) = components.next() else {
        return Some(path.to_owned());
    };
    let Some(user) = first.as_bytes().strip_prefix(b"~") else {
        return Some(path.to_owned());
    };
    let home = if user.is_empty() {
        env("HOME").map(PathBuf::from).or_else(|| user_home(None))
    } else {
        user_home(Some(OsStr::from_bytes(user)))
    }?;
    // Python treats an explicitly empty HOME (or account home) as "/".
    let mut expanded = if home.as_os_str().is_empty() {
        PathBuf::from("/")
    } else {
        home
    };
    expanded.extend(components);
    Some(expanded)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_selection_uses_launcher_precedence_without_creating_a_socket() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let home = root.join("home");
        std::fs::create_dir(&home).unwrap();
        std::os::unix::fs::symlink(&home, root.join("alias")).unwrap();
        let mut settings = HashMap::<&str, OsString>::new();
        settings.insert("HOME", home.as_os_str().to_owned());
        settings.insert("TMUX_TMPDIR", root.as_os_str().to_owned());
        let inherited = root.join("server,with,commas");
        let mut identity = inherited.as_os_str().to_owned();
        identity.push(",123,4");
        settings.insert("TMUX", identity);
        let select = |explicit: Option<&Path>, settings: &HashMap<&str, OsString>| {
            resolve_source(explicit, &|name| settings.get(name).cloned(), &|_| None, 42)
        };
        assert_eq!(select(None, &settings).unwrap(), inherited);
        assert_eq!(select(Some(Path::new("")), &settings).unwrap(), inherited);
        assert_eq!(
            select(Some(Path::new("~/chosen")), &settings).unwrap(),
            home.join("chosen")
        );
        assert_eq!(
            select(Some(&root.join("alias/missing/socket")), &settings).unwrap(),
            home.join("missing/socket")
        );
        assert_eq!(
            select(Some(Path::new("relative-source")), &settings).unwrap(),
            std::env::current_dir().unwrap().join("relative-source")
        );
        settings.insert("TMUX", "malformed,identity".into());
        assert_eq!(
            select(None, &settings).unwrap(),
            root.join("tmux-42/default")
        );
        settings.insert("TMUX_TMPDIR", "".into());
        assert_eq!(
            select(None, &settings).unwrap(),
            Path::new("/tmp")
                .canonicalize()
                .unwrap()
                .join("tmux-42/default")
        );
        assert!(!inherited.exists());
        assert!(!home.join("chosen").exists());
        assert!(!home.join("missing").exists());
        assert!(!root.join("tmux-42").exists());
    }

    #[test]
    fn library_paths_follow_precedence_and_resolve_without_creating_directories() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let home = root.join("home");
        let default = home.join(".local/share/tmux-workspaces");
        let xdg = root.join("data");
        let configured = root.join("configured library");
        let explicit = root.join("explicit library");
        let other = root.join("other");
        let raw = root.join(OsStr::from_bytes(b"library-\xff"));
        let target = root.join("nested/child");
        let sibling = root.join("nested/explicit library");
        for path in [
            &default,
            &xdg.join("tmux-workspaces"),
            &configured,
            &explicit,
            &other,
            &target,
            &sibling,
        ] {
            std::fs::create_dir_all(path).unwrap();
        }
        let account_home = |name: Option<&OsStr>| match name {
            None => Some(home.clone()),
            Some(name) if name == "alice" => Some(other.clone()),
            _ => None,
        };
        let locate = |path: Option<&Path>, settings: &HashMap<&str, OsString>| {
            resolve(path, &|name| settings.get(name).cloned(), &account_home)
        };
        let mut settings = HashMap::new();
        assert_eq!(locate(None, &settings).unwrap(), default);
        settings.insert("XDG_DATA_HOME", xdg.into_os_string());
        assert_eq!(
            locate(None, &settings).unwrap(),
            root.join("data/tmux-workspaces")
        );
        settings.insert(
            "TMUX_WORKSPACES_DATA_DIR",
            configured.clone().into_os_string(),
        );
        assert_eq!(locate(None, &settings).unwrap(), configured);
        assert_eq!(locate(Some(&explicit), &settings).unwrap(), explicit);
        assert_eq!(library_directory(Some(&explicit)).unwrap(), explicit);
        settings.insert("TMUX_WORKSPACES_DATA_DIR", "".into());
        settings.insert("XDG_DATA_HOME", "".into());
        settings.insert("HOME", home.clone().into_os_string());
        assert_eq!(locate(None, &settings).unwrap(), default);
        assert_eq!(
            locate(
                Some(Path::new("./~/.local/share/tmux-workspaces")),
                &settings
            )
            .unwrap(),
            default
        );
        assert_eq!(locate(Some(Path::new("~alice")), &settings).unwrap(), other);
        assert!(locate(Some(Path::new("~missing-user")), &settings).is_err());
        assert_eq!(
            expand_home(&raw, &|name| settings.get(name).cloned(), &account_home).unwrap(),
            raw
        );
        // APFS rejects invalid UTF-8 filenames; Linux filesystems can retain them.
        #[cfg(target_os = "linux")]
        {
            std::fs::create_dir(&raw).unwrap();
            assert_eq!(locate(Some(&raw), &settings).unwrap(), raw);
        }
        let link = root.join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert_eq!(
            locate(Some(&link.join("../explicit library")), &settings).unwrap(),
            sibling
        );
        assert_eq!(
            locate(Some(Path::new("")), &settings).unwrap(),
            std::env::current_dir().unwrap().canonicalize().unwrap()
        );
        let missing = root.join("missing");
        assert!(locate(Some(&missing), &settings).is_err());
        assert!(!missing.exists());
        assert_eq!(
            locate(Some(&missing.join("../explicit library")), &settings).unwrap(),
            explicit
        );
        assert_eq!(
            locate(
                Some(&missing.join("../link/../explicit library")),
                &settings
            )
            .unwrap(),
            sibling
        );
        assert!(!missing.exists());
        let indirect = root.join("indirect");
        std::os::unix::fs::symlink("missing/../nested/child", &indirect).unwrap();
        assert_eq!(locate(Some(&indirect), &settings).unwrap(), target);
        let cycle = root.join("cycle");
        std::os::unix::fs::symlink("cycle", &cycle).unwrap();
        assert!(locate(Some(&cycle), &settings).is_err());
        assert!(locate(Some(&root.join("bad\0/../explicit library")), &settings).is_err());
        settings.insert("HOME", "".into());
        assert_eq!(
            expand_home(
                Path::new("~/data"),
                &|name| settings.get(name).cloned(),
                &account_home
            )
            .unwrap(),
            Path::new("/data")
        );
    }
}
