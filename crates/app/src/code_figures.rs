//! Git diffs and code shown inline in a Document. The Document holds only a
//! pointer to commits; the app reads them from a local checkout and puts a
//! static view in place of the pointer before the page is served. A pointer
//! Git cannot answer stays exactly as authored. No GUI.

use std::{
    collections::HashMap,
    mem::size_of,
    path::PathBuf,
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
    },
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

/// When the work for a page stops: at its deadline, or as soon as the page
/// is no longer wanted.
#[derive(Clone, Debug)]
pub struct Until {
    at: Instant,
    cancelled: Arc<AtomicBool>,
}

impl Until {
    /// Until `at`, unless cancelled first.
    pub fn new(at: Instant) -> Self {
        Self {
            at,
            cancelled: Default::default(),
        }
    }

    /// Stop the work, and all work bounded by a clone, at its next check.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    /// Whether the work must stop now.
    pub fn passed(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed) || Instant::now() >= self.at
    }

    /// The deadline.
    pub fn at(&self) -> Instant {
        self.at
    }
}

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

/// Keep `read` under `key`, charged once even when another read stored the
/// same answer meanwhile.
fn remember(cache: &mut (Answers, usize), key: (PathBuf, Question), read: Arc<Read>) {
    let charge = |read: &Read| {
        read.size() + size_of::<(PathBuf, Question)>() + key.0.capacity() + key.1.held()
    };
    if let Some(old) = cache.0.remove(&key) {
        cache.1 = cache.1.saturating_sub(charge(&old));
    }
    let size = charge(&read);
    if cache.1 + size > CACHE_LIMIT {
        *cache = Default::default();
    }
    cache.1 += size;
    cache.0.insert(key, read);
}

/// What Git reads for a pointer, all its answer depends on: not the
/// repository's name, already matched to the clone, nor a code pointer's
/// lines, chosen from the whole file afterwards.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Question {
    Diff {
        base: String,
        head: String,
        paths: Option<Vec<String>>,
    },
    File {
        commit: String,
        path: String,
    },
}

impl Question {
    fn of(pointer: &Pointer) -> Self {
        match pointer {
            Pointer::Diff {
                base, head, paths, ..
            } => Self::Diff {
                base: base.clone(),
                head: head.clone(),
                paths: paths.clone(),
            },
            Pointer::Code { commit, path, .. } => Self::File {
                commit: commit.clone(),
                path: path.clone(),
            },
        }
    }

    /// The memory its strings hold.
    fn held(&self) -> usize {
        match self {
            Self::Diff { base, head, paths } => {
                let paths = paths.as_ref().map_or(0, |paths| {
                    paths.capacity() * size_of::<String>()
                        + paths.iter().map(String::capacity).sum::<usize>()
                });
                base.capacity() + head.capacity() + paths
            }
            Self::File { commit, path } => commit.capacity() + path.capacity(),
        }
    }
}

/// Answers by the clone (its common Git directory) that gave them.
type Answers = HashMap<(PathBuf, Question), Arc<Read>>;

/// The session's answers and the memory they hold.
static CACHE: LazyLock<Mutex<(Answers, usize)>> = LazyLock::new(Default::default);

/// `html` with a view in place of each pointer's content, or `None` when no
/// pointer could be read. Checkouts are looked for in `folders`: workspace
/// and agent folders. Git runs until `deadline` at the latest; a pointer not
/// read by then stays as authored. This blocks; call it off the UI thread.
pub fn transform(html: &str, folders: &[PathBuf], until: &Until) -> Option<String> {
    let found = pointer::find(html, until);
    if found.is_empty() {
        return None;
    }
    let checkouts = checkout::checkouts(folders, until);
    let mut views = Vec::new();
    let mut added = 0;
    for found in found {
        let Some(read) = answer(&found.pointer, &checkouts, until) else {
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
fn answer(pointer: &Pointer, checkouts: &[Checkout], until: &Until) -> Option<Arc<Read>> {
    let commits: Vec<&str> = match pointer {
        Pointer::Diff { base, head, .. } => vec![base, head],
        Pointer::Code { commit, .. } => vec![commit],
    };
    let cache = || CACHE.lock().unwrap_or_else(|error| error.into_inner());
    for checkout in checkouts
        .iter()
        .filter(|checkout| checkout.is(pointer.repo()))
    {
        // Even a cached answer needs the commits here now: the folder may
        // hold another clone since.
        let top = checkout.top();
        if !commits
            .iter()
            .all(|commit| git::has_commit(top, commit, until))
        {
            continue;
        }
        let key = (checkout.common().to_owned(), Question::of(pointer));
        if let Some(read) = cache().0.get(&key).cloned() {
            return Some(read);
        }
        let read = Arc::new(match pointer {
            Pointer::Diff {
                base, head, paths, ..
            } => Read::Diff(git::diff(top, base, head, paths.as_deref(), until)?),
            Pointer::Code { commit, path, .. } => Read::Code(git::file(top, commit, path, until)?),
        });
        remember(&mut cache(), key, read.clone());
        return Some(read);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::git::tests::{git, repository};
    use super::*;
    use std::time::Duration;

    fn later() -> Until {
        Until::new(Instant::now() + Duration::from_secs(10))
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
        let out = transform(&html, &folders, &later()).unwrap();
        assert!(out.starts_with(&format!("{before}<figure data-backbone-diff=")));
        assert!(out.ends_with("</figure><p>End</p>"));
        assert!(out.contains(
            r#"<details open class="ai-git"><summary>The <b>fix</b></summary><div class="ai-git-head">a.txt</div><div class="ai-git-scroll">"#
        ));
        assert!(out.contains(r#"<td class="ai-git-s">+</td><td class="ai-git-t">TWO</td>"#));
        assert!(out.contains("<summary>a.txt · lines 2–2</summary>"));
        assert!(!out.contains("<figcaption>"));
        // No script is added: the page's own is still the only one.
        assert_eq!(out.matches("<script").count(), 1);
        // An answer is kept: with the file's contents gone from the checkout,
        // only the cache can give the same page again. But only a checkout
        // at hand with the commits gives it.
        for blob in [format!("{first}:a.txt"), format!("{second}:a.txt")] {
            let id = git(&dir, &["rev-parse", &blob]);
            std::fs::remove_file(dir.join(".git/objects").join(&id[..2]).join(&id[2..])).unwrap();
        }
        assert_eq!(transform(&html, &folders, &later()).as_ref(), Some(&out));
        assert_eq!(transform(&html, &[], &later()), None);
        std::fs::remove_dir_all(dir.join(".git")).unwrap();
        git(&dir, &["init", "-q"]);
        git(
            &dir,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/Owner/Name.git",
            ],
        );
        assert_eq!(transform(&html, &folders, &later()), None);
    }

    #[test]
    fn cancelling_stops_a_running_transform() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("checkout");
        std::fs::create_dir(&dir).unwrap();
        let (_, second) = repository(&dir, "git@github.com:owner/name.git");
        // Git waits on this configuration file until it is stopped.
        let fifo = root.path().join("fifo");
        let made = std::process::Command::new("mkfifo").arg(&fifo).status();
        assert!(made.unwrap().success());
        git(&dir, &["config", "include.path", fifo.to_str().unwrap()]);
        let html = format!(
            r#"<figure data-backbone-code="1" data-repo="owner/name" data-commit="{second}" data-path="a.txt"></figure>"#
        );
        let until = later();
        let cancel = until.clone();
        let started = Instant::now();
        let stopper = std::thread::spawn(move || {
            use std::os::unix::fs::OpenOptionsExt;
            // A writer can open the file without waiting only once Git has
            // it open to read: Git is running and waiting on it.
            let opened = Instant::now();
            let writer = loop {
                let open = std::fs::OpenOptions::new()
                    .write(true)
                    .custom_flags(libc::O_NONBLOCK)
                    .open(&fifo);
                match open {
                    Ok(writer) => break writer,
                    Err(_) if opened.elapsed() < Duration::from_secs(5) => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("Git never read the file: {error}"),
                }
            };
            cancel.cancel();
            // Held open, so Git is still waiting when it is stopped.
            writer
        });
        assert_eq!(transform(&html, &[dir], &until), None);
        // Well before the 3 s a single command may take.
        assert!(started.elapsed() < Duration::from_secs(2));
        drop(stopper.join().unwrap());
    }

    #[test]
    fn an_answer_stored_twice_is_charged_once() {
        let mut cache = (Answers::new(), 0);
        let question = Question::File {
            commit: "0".repeat(40),
            path: "a.txt".into(),
        };
        let key = (PathBuf::from("/clone/.git"), question);
        let read = || Arc::new(Read::Code("text".into()));
        remember(&mut cache, key.clone(), read());
        let once = cache.1;
        remember(&mut cache, key, read());
        assert_eq!((cache.0.len(), cache.1), (1, once));
    }

    #[test]
    fn excerpts_of_one_file_share_one_read() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("checkout");
        std::fs::create_dir(&dir).unwrap();
        let (_, second) = repository(&dir, "https://github.com/owner/name");
        let excerpt = |repo: &str, lines: &str| {
            format!(
                r#"<figure data-backbone-code="1" data-repo="{repo}" data-commit="{second}" data-path="a.txt" data-lines="{lines}"></figure>"#
            )
        };
        let html = format!(
            "{}{}",
            excerpt("owner/name", "1"),
            excerpt("Owner/Name", "2-3")
        );
        let out = transform(&html, std::slice::from_ref(&dir), &later()).unwrap();
        assert!(out.contains("lines 1–1") && out.contains("lines 2–3"));
        let clone = checkout::checkouts(&[dir], &later())[0].common().to_owned();
        let cache = CACHE.lock().unwrap_or_else(|error| error.into_inner());
        assert_eq!(
            cache
                .0
                .keys()
                .filter(|(common, _)| *common == clone)
                .count(),
            1
        );
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
            assert_eq!(transform(&html, &folders, &later()), None, "{html}");
        }
        assert!(transform(&pointer("owner/name", &second), &folders, &later()).is_some());
    }
}
