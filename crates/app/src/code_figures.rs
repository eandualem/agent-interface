//! Git diffs and code shown inline in a Document. The Document holds only a
//! pointer to commits; the app reads them from a local checkout and puts a
//! static view in place of the pointer before the page is served. A pointer
//! Git cannot answer stays exactly as authored. No GUI.

use std::{
    collections::HashMap,
    mem::size_of,
    path::PathBuf,
    sync::{Arc, LazyLock, Mutex},
    time::Instant,
};

pub mod checkout;
pub mod git;
pub mod pointer;
pub mod render;

use checkout::Checkout;
use pointer::Pointer;

/// The most view HTML added to one page; later pointers stay as authored.
const VIEW_LIMIT: usize = 32 * 1024 * 1024;
/// The most memory the session's answers may hold. Commits never change, so
/// an answer is kept until the cache is full, and then all are dropped.
const CACHE_LIMIT: usize = 64 * 1024 * 1024;

/// What Git answered for a pointer.
enum Read {
    Diff(Vec<git::File>),
    Code(String),
}

impl Read {
    /// The memory it holds, roughly.
    fn size(&self) -> usize {
        match self {
            Self::Diff(files) => {
                let row = |row: &git::Row| match row {
                    git::Row::Hunk(text) | git::Row::Remark(text) => text.capacity(),
                    git::Row::Line { text, .. } => text.capacity(),
                    git::Row::Binary => 0,
                };
                let file = |file: &git::File| {
                    file.path.capacity()
                        + file.from.as_ref().map_or(0, String::capacity)
                        + file.rows.capacity() * size_of::<git::Row>()
                        + file.rows.iter().map(row).sum::<usize>()
                };
                files.capacity() * size_of::<git::File>() + files.iter().map(file).sum::<usize>()
            }
            Self::Code(text) => text.capacity(),
        }
    }
}

/// Answers by the clone (its common Git directory) that gave them.
type Answers = HashMap<(PathBuf, Pointer), Arc<Read>>;

/// The session's answers and the memory they hold.
static CACHE: LazyLock<Mutex<(Answers, usize)>> = LazyLock::new(Default::default);

/// `html` with a view in place of each pointer's content, or `None` when no
/// pointer could be read. Checkouts are looked for in `folders`: workspace
/// and agent folders. Git runs until `deadline` at the latest; a pointer not
/// read by then stays as authored. This blocks; call it off the UI thread.
pub fn transform(html: &str, folders: &[PathBuf], deadline: Instant) -> Option<String> {
    let found = pointer::find(html);
    if found.is_empty() {
        return None;
    }
    let checkouts = checkout::checkouts(folders, deadline);
    let mut views = Vec::new();
    let mut added = 0;
    for found in found {
        let Some(read) = answer(&found.pointer, &checkouts, deadline) else {
            continue;
        };
        let caption = found.caption.map(|range| &html[range]);
        let room = VIEW_LIMIT - added;
        let view = match (&found.pointer, &*read) {
            (Pointer::Diff { .. }, Read::Diff(files)) => render::diff(files, caption, room),
            (Pointer::Code { path, lines, .. }, Read::Code(text)) => {
                render::code(path, text, *lines, caption, room)
            }
            _ => None,
        };
        let Some(view) = view else { continue };
        added += view.len();
        views.push((found.content, view));
    }
    (!views.is_empty()).then(|| pointer::splice(html, &views))
}

/// What the first checkout of the pointer's repository with its commits
/// answers. An answer that checkout gave before is taken from the cache.
fn answer(pointer: &Pointer, checkouts: &[Checkout], deadline: Instant) -> Option<Arc<Read>> {
    let commits: Vec<&str> = match pointer {
        Pointer::Diff { base, head, .. } => vec![base, head],
        Pointer::Code { commit, .. } => vec![commit],
    };
    let cache = || CACHE.lock().unwrap_or_else(|error| error.into_inner());
    for checkout in checkouts
        .iter()
        .filter(|checkout| checkout.is(pointer.repo()))
    {
        let key = (checkout.common().to_owned(), pointer.clone());
        if let Some(read) = cache().0.get(&key).cloned() {
            return Some(read);
        }
        let top = checkout.top();
        if !commits
            .iter()
            .all(|commit| git::has_commit(top, commit, deadline))
        {
            continue;
        }
        let read = Arc::new(match pointer {
            Pointer::Diff {
                base, head, paths, ..
            } => Read::Diff(git::diff(top, base, head, paths.as_deref(), deadline)?),
            Pointer::Code { commit, path, .. } => {
                Read::Code(git::file(top, commit, path, deadline)?)
            }
        });
        let size = read.size() + size_of::<(PathBuf, Pointer)>() + key.0.capacity();
        let mut cache = cache();
        if cache.1 + size > CACHE_LIMIT {
            *cache = Default::default();
        }
        cache.1 += size;
        cache.0.insert(key, read.clone());
        return Some(read);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::git::tests::{git, repository};
    use super::*;
    use std::time::Duration;

    fn later() -> Instant {
        Instant::now() + Duration::from_secs(10)
    }

    #[test]
    fn pointers_are_replaced_by_views_and_nothing_else_changes() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("checkout");
        std::fs::create_dir(&dir).unwrap();
        let (first, second) = repository(&dir, "https://github.com/Owner/Name.git");
        let before = "<!doctype html><script>let a = 1;</script><p>Intro</p>";
        let html = format!(
            r#"{before}<figure data-backbone-diff="1" data-repo="owner/name" data-base="{first}" data-head="{second}" data-paths='["a.txt"]'><figcaption>The <b>fix</b></figcaption></figure><figure data-backbone-code="1" data-repo="owner/name" data-commit="{second}" data-path="a.txt" data-lines="2"></figure><p>End</p>"#
        );
        let folders = [
            root.path().join("missing"),
            dir.join("sub-folder-that-is-absent"),
            dir.clone(),
        ];
        let out = transform(&html, &folders, later()).unwrap();
        assert!(out.starts_with(&format!("{before}<figure data-backbone-diff=")));
        assert!(out.ends_with("</figure><p>End</p>"));
        assert!(out.contains(
            r#"<details open class="ai-git"><summary>The <b>fix</b></summary><div class="ai-git-scroll">"#
        ));
        assert!(out.contains(r#"<td class="ai-git-s">+</td><td class="ai-git-t">TWO</td>"#));
        assert!(out.contains("<summary>a.txt · lines 2–2</summary>"));
        assert!(!out.contains("<figcaption>"));
        // No script is added: the page's own is still the only one.
        assert_eq!(out.matches("<script").count(), 1);
        // An answer is kept, but only a checkout at hand gives it again.
        assert_eq!(transform(&html, &folders, later()).as_ref(), Some(&out));
        assert_eq!(transform(&html, &[], later()), None);
    }

    #[test]
    fn a_pointer_git_cannot_answer_stays_as_authored() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("checkout");
        std::fs::create_dir(&dir).unwrap();
        let (_, second) = repository(&dir, "git@github.com:owner/name.git");
        git(
            &dir,
            &[
                "remote",
                "add",
                "other",
                "git@github.com:owner/elsewhere.git",
            ],
        );
        let pointer = |repo: &str, commit: &str| {
            format!(
                r#"<figure data-backbone-code="1" data-repo="{repo}" data-commit="{commit}" data-path="a.txt"></figure>"#
            )
        };
        let folders = [dir.clone()];
        for html in [
            // No checkout of that repository, or the commit is not in it.
            pointer("owner/elsewhere", &second),
            pointer("owner/name", &"f".repeat(40)),
            // Not a pointer.
            pointer("owner/name", &second[..12]),
            "<p>No pointers</p>".to_owned(),
        ] {
            assert_eq!(transform(&html, &folders, later()), None, "{html}");
        }
        assert!(transform(&pointer("owner/name", &second), &folders, later()).is_some());
    }
}
