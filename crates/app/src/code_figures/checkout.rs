//! Local checkouts of a GitHub repository among the folders the app knows:
//! a folder matches when its origin remote is that repository.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use super::{Until, git};

/// The most folders looked at for one Document.
pub const LIMIT: usize = 64;

/// A folder's checkout: its working tree's top level, the Git directory its
/// worktrees share, and its origin remote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checkout {
    top: PathBuf,
    /// The common Git directory, the same for every worktree of a clone.
    common: PathBuf,
    origin: Option<String>,
}

/// The checkouts that `folders` are in, one per clone, in the folders' order.
/// Folders that are not in a checkout are left out, and so are partial
/// clones, which Git would fill in from the network.
pub fn checkouts(folders: &[PathBuf], until: &Until) -> Vec<Checkout> {
    let mut seen = HashSet::new();
    let mut found: Vec<Checkout> = Vec::new();
    let folders = folders
        .iter()
        .filter(|folder| seen.insert(*folder))
        .take(LIMIT);
    for folder in folders {
        if until.passed() {
            break;
        }
        if !folder.is_absolute() || !folder.is_dir() {
            continue;
        }
        let Some(out) = git::run(
            folder,
            &[
                "rev-parse",
                "--path-format=absolute",
                "--show-toplevel",
                "--git-common-dir",
            ],
            64 * 1024,
            until,
        ) else {
            continue;
        };
        let Ok(out) = String::from_utf8(out) else {
            continue;
        };
        let mut lines = out.lines();
        let (Some(top), Some(common)) = (lines.next(), lines.next()) else {
            continue;
        };
        let common = PathBuf::from(common);
        if found.iter().any(|checkout| checkout.common == common) {
            continue;
        }
        let top = PathBuf::from(top);
        // The origin as stored, not as `url.*.insteadOf` would rewrite it.
        let config = git::run(
            &top,
            &[
                "config",
                "-z",
                "--get-regexp",
                r"^(remote\.origin\.url|extensions\.(partialclone|objectformat)|remote\..*\.(promisor|partialclonefilter))$",
            ],
            64 * 1024,
            until,
        )
        .unwrap_or_default();
        let mut origin = None;
        // A partial clone, or one whose object IDs are not SHA-1, is refused.
        let mut refused = false;
        for entry in config
            .split(|&byte| byte == 0)
            .filter(|entry| !entry.is_empty())
        {
            match String::from_utf8_lossy(entry).split_once('\n') {
                Some(("remote.origin.url", url)) => {
                    origin.get_or_insert_with(|| url.trim().to_owned());
                }
                Some(("extensions.objectformat", format))
                    if format.trim().eq_ignore_ascii_case("sha1") => {}
                _ => refused = true,
            }
        }
        if !refused {
            found.push(Checkout {
                top,
                common,
                origin,
            });
        }
    }
    found
}

impl Checkout {
    /// Whether its origin is the GitHub repository `repo` (`owner/name`).
    pub fn is(&self, repo: &str) -> bool {
        self.origin
            .as_deref()
            .is_some_and(|url| is_remote(url, repo))
    }

    /// Its working tree's top level, where it is read.
    pub fn top(&self) -> &Path {
        &self.top
    }

    /// The Git directory its worktrees share, which names the clone.
    pub fn common(&self) -> &Path {
        &self.common
    }
}

/// Whether `url` names the GitHub repository `repo` (`owner/name`), in its
/// HTTPS or SSH form, ignoring case, a `.git` suffix and a final `/`.
pub fn is_remote(url: &str, repo: &str) -> bool {
    let url = url.trim();
    let scheme = ["https://", "ssh://"].into_iter().find(|scheme| {
        url.get(..scheme.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(scheme))
    });
    let (authority, path) = match scheme {
        Some(scheme) => match url[scheme.len()..].split_once('/') {
            Some(parts) => parts,
            None => return false,
        },
        // `user@host:path`; a local path has a `/` before any `:`.
        None => match url.split_once(':') {
            Some((authority, path)) if !authority.contains('/') => (authority, path),
            _ => return false,
        },
    };
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let (host, port) = match host.split_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (host, None),
    };
    // These forms only; GitHub also serves SSH on port 443 at ssh.github.com.
    let github = match (scheme, port) {
        (Some("https://") | None, None) | (Some("ssh://"), None | Some("22")) => "github.com",
        (Some("ssh://"), Some("443")) => "ssh.github.com",
        _ => return false,
    };
    let path = path.trim_start_matches('/').trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    host.eq_ignore_ascii_case(github) && path.eq_ignore_ascii_case(repo)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code_figures::git::tests::{git, repository};
    use std::time::{Duration, Instant};

    #[test]
    fn remotes_match_in_every_github_form() {
        for url in [
            "https://github.com/owner/name",
            "https://github.com/owner/name.git",
            "https://github.com/owner/name/",
            "HTTPS://GitHub.com/Owner/Name.git",
            "https://user:token@github.com/owner/name.git",
            "ssh://git@github.com/owner/name.git",
            "ssh://git@github.com:22/owner/name",
            "ssh://git@ssh.github.com:443/owner/name.git",
            "git@github.com:owner/name.git",
            "git@github.com:/owner/name",
            "github.com:owner/name",
            " git@github.com:owner/name.git\n",
        ] {
            assert!(is_remote(url, "owner/name"), "{url}");
        }
        for url in [
            "https://gitlab.com/owner/name.git",
            "https://github.com.example.com/owner/name",
            "https://examplegithub.com/owner/name",
            "https://github.com/owner/name-other",
            "https://github.com/owner/nam",
            "https://github.com/other/owner/name",
            "git@github.com:owner/name/extra",
            "/srv/github.com:owner/name",
            "file:///github.com/owner/name",
            "github.com",
            "",
            // No other forms.
            "http://github.com/owner/name",
            "git://github.com/owner/name",
            "https://github.com:8443/owner/name",
            "ssh://git@github.com:2222/owner/name",
            "ssh://git@ssh.github.com/owner/name",
            "git@ssh.github.com:owner/name",
        ] {
            assert!(!is_remote(url, "owner/name"), "{url}");
        }
    }

    #[test]
    fn folders_resolve_to_one_checkout_per_clone() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let (main, other, plain) = (root.join("main"), root.join("other"), root.join("plain"));
        for dir in [&main, &other, &plain] {
            std::fs::create_dir(dir).unwrap();
        }
        repository(&main, "git@github.com:owner/name.git");
        repository(&other, "https://github.com/someone/else");
        let partial = root.join("partial");
        std::fs::create_dir(&partial).unwrap();
        repository(&partial, "git@github.com:owner/name.git");
        git(&partial, &["config", "remote.origin.promisor", "true"]);
        let sha256 = root.join("sha256");
        std::fs::create_dir(&sha256).unwrap();
        git(&sha256, &["init", "-q", "--object-format=sha256"]);
        git(
            &sha256,
            &["remote", "add", "origin", "git@github.com:owner/name.git"],
        );
        let worktree = root.join("worktree");
        git(
            &main,
            &["worktree", "add", "-q", worktree.to_str().unwrap()],
        );
        let mut folders = vec![plain.clone(); 70];
        folders.extend([
            partial,
            sha256,
            root.join("missing"),
            PathBuf::from("relative"),
            main.join("sub"),
            worktree.clone(),
            main.clone(),
            other.clone(),
        ]);
        std::fs::create_dir(main.join("sub")).unwrap();
        assert_eq!(checkouts(&folders, &Until::new(Instant::now())), vec![]);
        let found = checkouts(
            &folders,
            &Until::new(Instant::now() + Duration::from_secs(10)),
        );
        // Repeated folders count once. The partial and SHA-256 clones are left
        // out, and the worktree shares its clone with `main`, so only the first
        // is kept.
        assert_eq!(
            found.iter().map(Checkout::top).collect::<Vec<_>>(),
            [main.as_path(), other.as_path()]
        );
        assert!(found[0].is("owner/name") && !found[0].is("someone/else"));
        assert!(found[1].is("someone/else"));
    }
}
