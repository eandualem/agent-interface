//! Read-only Git commands, each with a deadline and an output bound, and the
//! two reads a pointer needs: a diff of two commits and a file at a commit.
//! Nothing here writes to a checkout, runs a configured program or fetches.

use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

/// The most output one Git command may write.
pub const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;
/// The longest one Git command may run.
const TIMEOUT: Duration = Duration::from_secs(3);
/// The most rows read from one diff, and lines shown from one file.
pub const ROW_LIMIT: usize = 20_000;

/// The standard output of `git -C dir args…`, when it succeeds by `deadline`
/// (and within 3 s) and writes at most `limit` bytes.
pub fn run(dir: &Path, args: &[&str], limit: usize, deadline: Instant) -> Option<Vec<u8>> {
    if Instant::now() >= deadline {
        return None;
    }
    let mut command = Command::new("git");
    // No inherited Git environment chooses another repository or config.
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("GIT_") {
            command.env_remove(name);
        }
    }
    command
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        // A partial clone would otherwise fetch a missing object; no
        // transport may run at all.
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_ALLOW_PROTOCOL", "")
        // A commit ID names that commit, not a local replacement.
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .arg("-C")
        .arg(dir)
        .args(["-c", "core.pager=cat", "-c", "protocol.allow=never"])
        .arg("--literal-pathspecs")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = thread::spawn(move || {
        let mut out = Vec::new();
        let read = (&mut stdout).take(limit as u64 + 1).read_to_end(&mut out);
        read.ok().filter(|_| out.len() <= limit).map(|_| out)
    });
    let deadline = deadline.min(Instant::now() + TIMEOUT);
    let stop = |child: &mut std::process::Child| {
        let _ = child.kill();
        let _ = child.wait();
    };
    while !reader.is_finished() {
        if Instant::now() >= deadline {
            stop(&mut child);
            let _ = reader.join();
            return None;
        }
        thread::sleep(Duration::from_millis(2));
    }
    let Some(out) = reader.join().ok().flatten() else {
        stop(&mut child);
        return None;
    };
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success().then_some(out),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(2)),
            _ => {
                stop(&mut child);
                return None;
            }
        }
    }
}

/// Whether the checkout at `top` has `commit`, and it is a commit rather
/// than, say, a tag.
pub fn has_commit(top: &Path, commit: &str, deadline: Instant) -> bool {
    run(top, &["cat-file", "-t", commit], 64, deadline).as_deref() == Some(&b"commit\n"[..])
}

/// One changed file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct File {
    /// Git's status letter: `A`, `D`, `M`, `R` or `T`.
    pub status: char,
    /// The path before a rename.
    pub from: Option<String>,
    /// The path.
    pub path: String,
    /// Hunk headers and lines, in order.
    pub rows: Vec<Row>,
}

/// One row of a file's diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    /// A hunk header, such as `@@ -1,3 +1,4 @@ fn main()`.
    Hunk(String),
    /// A line, with its number on each side it exists on.
    Line {
        /// `' '`, `'+'` or `'-'`.
        sign: char,
        /// Its number in the old file.
        old: Option<u32>,
        /// Its number in the new file.
        new: Option<u32>,
        /// The line, without its sign or line end.
        text: String,
    },
    /// Git's remark on the line above, such as `\ No newline at end of file`.
    Remark(String),
    /// The file is binary, so it has no lines to show.
    Binary,
}

/// `git diff BASE HEAD` of the two commits' trees, limited to `paths`. A
/// diff that touches a submodule is not read.
pub fn diff(
    top: &Path,
    base: &str,
    head: &str,
    paths: Option<&[String]>,
    deadline: Instant,
) -> Option<Vec<File>> {
    // Attributes, such as which files are binary, come from the new commit,
    // not from whatever the checkout has; a Git without this option answers
    // nothing.
    let attributes = format!("--attr-source={head}");
    let mut args = vec![
        &*attributes,
        "diff",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "-M",
        "-U3",
    ];
    let mut tail = vec![base, head, "--"];
    tail.extend(paths.unwrap_or_default().iter().map(String::as_str));
    // The file list, with exact names, then the patch in the same order.
    let raw = run(
        top,
        &[
            &args[..],
            &["--raw", "-z", "--ignore-submodules=none"][..],
            &tail[..],
        ]
        .concat(),
        OUTPUT_LIMIT,
        deadline,
    )?;
    let entries = entries(&raw)?;
    // A submodule's patch could run Git inside it.
    if entries.iter().any(|entry| entry.submodule) {
        return None;
    }
    args.extend(tail);
    let patch = run(top, &args, OUTPUT_LIMIT, deadline)?;
    files(entries, &patch)
}

/// One file in a `--raw -z` list.
struct Entry {
    status: char,
    from: Option<String>,
    path: String,
    /// Whether either side is a submodule commit.
    submodule: bool,
}

/// The files of a `--raw -z` list: for each, `:old-mode new-mode old-id
/// new-id status`, then one path, or two for a rename.
fn entries(raw: &[u8]) -> Option<Vec<Entry>> {
    let mut fields = raw.split(|&byte| byte == 0);
    let mut entries = Vec::new();
    while let Some(meta) = fields.next().filter(|meta| !meta.is_empty()) {
        let meta = std::str::from_utf8(meta).ok()?;
        let mut parts = meta.strip_prefix(':')?.split(' ');
        let submodule = parts.by_ref().take(2).any(|mode| mode == "160000");
        let status = parts.next_back()?.chars().next()?;
        let mut next = || Some(String::from_utf8_lossy(fields.next()?).into_owned());
        let (from, path) = if matches!(status, 'R' | 'C') {
            (Some(next()?), next()?)
        } else {
            (None, next()?)
        };
        entries.push(Entry {
            status,
            from,
            path,
            submodule,
        });
    }
    Some(entries)
}

/// The files of a diff from its entries and its patch, in the same order.
fn files(entries: Vec<Entry>, patch: &[u8]) -> Option<Vec<File>> {
    let mut patches = sections(patch).into_iter();
    let mut files = Vec::new();
    let mut room = ROW_LIMIT;
    for entry in entries {
        // A change between a file and a link is shown as a deletion and a
        // creation.
        let mut rows = rows(patches.next()?, room)?;
        if entry.status == 'T' {
            rows.extend(self::rows(patches.next()?, room - rows.len())?);
        }
        room -= rows.len();
        files.push(File {
            status: entry.status,
            from: entry.from,
            path: entry.path,
            rows,
        });
    }
    patches.next().is_none().then_some(files)
}

/// A patch split into one part per `diff --git` header.
fn sections(patch: &[u8]) -> Vec<&[u8]> {
    let mut starts: Vec<usize> = Vec::new();
    let mut at = 0;
    for line in patch.split_inclusive(|&byte| byte == b'\n') {
        if line.starts_with(b"diff --git ") {
            starts.push(at);
        }
        at += line.len();
    }
    starts
        .iter()
        .enumerate()
        .map(|(index, &start)| &patch[start..starts.get(index + 1).copied().unwrap_or(patch.len())])
        .collect()
}

/// The rows of one file's patch, unless there are more than `room`. Header
/// lines before the first hunk are skipped; names come from the raw list.
fn rows(section: &[u8], room: usize) -> Option<Vec<Row>> {
    let mut rows = Vec::new();
    let (mut old, mut new) = (0, 0);
    let mut in_hunk = false;
    let section = section.strip_suffix(b"\n").unwrap_or(section);
    for line in section.split(|&byte| byte == b'\n').skip(1) {
        if rows.len() >= room {
            return None;
        }
        let text = || String::from_utf8_lossy(&line[1.min(line.len())..]).into_owned();
        if line.starts_with(b"@@ ") {
            let header = String::from_utf8_lossy(line).into_owned();
            let (from, to) = starts(&header);
            (old, new) = (from, to);
            rows.push(Row::Hunk(header));
            in_hunk = true;
        } else if !in_hunk {
            if line.starts_with(b"Binary files ") {
                rows.push(Row::Binary);
            }
        } else if let Some(&sign) = line.first() {
            match sign {
                b'+' => {
                    rows.push(Row::Line {
                        sign: '+',
                        old: None,
                        new: Some(new),
                        text: text(),
                    });
                    new += 1;
                }
                b'-' => {
                    rows.push(Row::Line {
                        sign: '-',
                        old: Some(old),
                        new: None,
                        text: text(),
                    });
                    old += 1;
                }
                b'\\' => rows.push(Row::Remark(String::from_utf8_lossy(line).into_owned())),
                _ => {
                    rows.push(Row::Line {
                        sign: ' ',
                        old: Some(old),
                        new: Some(new),
                        text: text(),
                    });
                    old += 1;
                    new += 1;
                }
            }
        } else {
            // An empty context line, as `diff.suppressBlankEmpty` writes it.
            rows.push(Row::Line {
                sign: ' ',
                old: Some(old),
                new: Some(new),
                text: String::new(),
            });
            old += 1;
            new += 1;
        }
    }
    Some(rows)
}

/// The first old and new line numbers in a hunk header `@@ -a,b +c,d @@`.
fn starts(header: &str) -> (u32, u32) {
    let mut numbers = header.split(' ').skip(1).take(2).map(|range| {
        range[1.min(range.len())..]
            .split(',')
            .next()
            .and_then(|number| number.parse().ok())
            .unwrap_or(0)
    });
    let old = numbers.next().unwrap_or(0);
    let new = numbers.next().unwrap_or(0);
    // A side with no lines (`-0,0`) names the line before the hunk.
    (old.max(1), new.max(1))
}

/// The text of `path` at `commit`, unless it is missing or binary.
pub fn file(top: &Path, commit: &str, path: &str, deadline: Instant) -> Option<String> {
    let blob = run(
        top,
        &["cat-file", "blob", &format!("{commit}:{path}")],
        OUTPUT_LIMIT,
        deadline,
    )?;
    if blob[..blob.len().min(8000)].contains(&0) {
        return None;
    }
    Some(String::from_utf8_lossy(&blob).into_owned())
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::{fs, path::PathBuf};

    /// Run Git for a test fixture, with no user or system configuration.
    pub fn git(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .args([
                "-c",
                "init.defaultBranch=main",
                "-c",
                "commit.gpgsign=false",
                "-C",
            ])
            .arg(dir)
            .args(args)
            .output()
            .expect("git runs");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    /// A repository with origin `remote`, and the IDs of its two commits.
    pub fn repository(dir: &Path, remote: &str) -> (String, String) {
        git(dir, &["init", "-q"]);
        git(dir, &["remote", "add", "origin", remote]);
        fs::write(dir.join("a.txt"), "one\ntwo\n\nthree\n").unwrap();
        fs::write(dir.join("old.txt"), "moved\n").unwrap();
        fs::write(dir.join("gone.txt"), "bye\n").unwrap();
        fs::write(dir.join("kind"), "file\n").unwrap();
        fs::write(dir.join("sp ace.txt"), "x\n").unwrap();
        fs::write(dir.join("bin.dat"), b"\0\x01\x02").unwrap();
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-qm", "first"]);
        let first = git(dir, &["rev-parse", "HEAD"]);
        fs::write(dir.join("a.txt"), "one\nTWO\n\nthree\n").unwrap();
        fs::rename(dir.join("old.txt"), dir.join("new.txt")).unwrap();
        fs::remove_file(dir.join("gone.txt")).unwrap();
        fs::remove_file(dir.join("kind")).unwrap();
        std::os::unix::fs::symlink("a.txt", dir.join("kind")).unwrap();
        fs::write(dir.join("sp ace.txt"), "y\n").unwrap();
        fs::write(dir.join("bin.dat"), b"\0\x03").unwrap();
        fs::write(dir.join("added.txt"), "new").unwrap();
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-qm", "second"]);
        (first, git(dir, &["rev-parse", "HEAD"]))
    }

    fn later() -> Instant {
        Instant::now() + Duration::from_secs(10)
    }

    fn line(sign: char, old: Option<u32>, new: Option<u32>, text: &str) -> Row {
        Row::Line {
            sign,
            old,
            new,
            text: text.into(),
        }
    }

    #[test]
    fn a_diff_reads_every_kind_of_change() {
        let dir = tempfile::tempdir().unwrap();
        let (first, second) = repository(dir.path(), "git@github.com:owner/name.git");
        // The checkout's own attributes do not change the answer.
        fs::write(dir.path().join(".gitattributes"), "a.txt -diff\n").unwrap();
        let files = diff(dir.path(), &first, &second, None, later()).unwrap();
        let by = |path: &str| files.iter().find(|file| file.path == path).unwrap();
        assert_eq!(files.len(), 7);
        assert_eq!(
            by("a.txt").rows,
            vec![
                Row::Hunk("@@ -1,4 +1,4 @@".into()),
                line(' ', Some(1), Some(1), "one"),
                line('-', Some(2), None, "two"),
                line('+', None, Some(2), "TWO"),
                line(' ', Some(3), Some(3), ""),
                line(' ', Some(4), Some(4), "three"),
            ]
        );
        assert_eq!(by("added.txt").status, 'A');
        assert_eq!(
            by("added.txt").rows,
            vec![
                Row::Hunk("@@ -0,0 +1 @@".into()),
                line('+', None, Some(1), "new"),
                Row::Remark("\\ No newline at end of file".into()),
            ]
        );
        assert_eq!(by("bin.dat").rows, vec![Row::Binary]);
        assert_eq!(by("gone.txt").status, 'D');
        assert_eq!(by("gone.txt").rows[1], line('-', Some(1), None, "bye"));
        let kind = by("kind");
        assert_eq!(kind.status, 'T');
        assert!(kind.rows.contains(&line('-', Some(1), None, "file")));
        assert!(kind.rows.contains(&line('+', None, Some(1), "a.txt")));
        let moved = by("new.txt");
        assert_eq!(
            (moved.status, moved.from.as_deref()),
            ('R', Some("old.txt"))
        );
        assert_eq!(moved.rows, vec![]);
        // Files after the split type change keep their own rows.
        assert_eq!(by("sp ace.txt").rows[2], line('+', None, Some(1), "y"));
    }

    #[test]
    fn a_diff_can_be_limited_to_paths() {
        let dir = tempfile::tempdir().unwrap();
        let (first, second) = repository(dir.path(), "git@github.com:owner/name.git");
        let paths = ["sp ace.txt".to_owned(), "missing/*".to_owned()];
        let files = diff(dir.path(), &first, &second, Some(&paths), later()).unwrap();
        assert_eq!(
            files.iter().map(|file| &*file.path).collect::<Vec<_>>(),
            ["sp ace.txt"]
        );
        // Paths are literal, never patterns.
        let star = ["*.txt".to_owned()];
        assert_eq!(
            diff(dir.path(), &first, &second, Some(&star), later()),
            Some(vec![])
        );
    }

    #[test]
    fn a_file_is_read_whole_and_missing_or_binary_files_are_not() {
        let dir = tempfile::tempdir().unwrap();
        let (first, second) = repository(dir.path(), "git@github.com:owner/name.git");
        assert_eq!(
            file(dir.path(), &second, "a.txt", later()).as_deref(),
            Some("one\nTWO\n\nthree\n")
        );
        assert_eq!(
            file(dir.path(), &first, "old.txt", later()).as_deref(),
            Some("moved\n")
        );
        assert_eq!(file(dir.path(), &second, "old.txt", later()), None);
        assert_eq!(file(dir.path(), &second, "bin.dat", later()), None);
        assert!(has_commit(dir.path(), &first, later()));
        assert!(!has_commit(dir.path(), &"0".repeat(40), later()));
        // A tag is not a commit, and a replacement never stands in for one.
        git(dir.path(), &["tag", "-a", "v1", "-m", "tag"]);
        assert!(!has_commit(
            dir.path(),
            &git(dir.path(), &["rev-parse", "v1"]),
            later()
        ));
        git(dir.path(), &["replace", &first, &second]);
        assert_eq!(
            file(dir.path(), &first, "old.txt", later()).as_deref(),
            Some("moved\n")
        );
        let missing = "0".repeat(40);
        assert_eq!(diff(dir.path(), &missing, &second, None, later()), None);
    }

    #[test]
    fn a_diff_touching_a_submodule_is_not_read() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        let mut commits = Vec::new();
        for id in ["a".repeat(40), "b".repeat(40)] {
            let entry = format!("160000,{id},sub");
            git(
                dir.path(),
                &["update-index", "--add", "--cacheinfo", &entry],
            );
            git(dir.path(), &["commit", "-qm", "sub"]);
            commits.push(git(dir.path(), &["rev-parse", "HEAD"]));
        }
        // Even where the configuration would hide submodules.
        git(dir.path(), &["config", "diff.ignoreSubmodules", "all"]);
        assert_eq!(
            diff(dir.path(), &commits[0], &commits[1], None, later()),
            None
        );
    }

    #[test]
    fn a_diff_with_too_many_rows_is_not_read() {
        let raw = b":100644 100644 0000000 1111111 M\0x\0";
        let patch = |count: usize| {
            format!(
                "diff --git a/x b/x\n@@ -0,0 +1 @@\n{}",
                "+a\n".repeat(count)
            )
        };
        let read = |count| files(entries(raw).unwrap(), patch(count).as_bytes());
        assert!(read(ROW_LIMIT - 1).is_some());
        assert_eq!(read(ROW_LIMIT), None);
    }

    #[test]
    fn output_and_time_are_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let (_, second) = repository(dir.path(), "git@github.com:owner/name.git");
        let blob = format!("{second}:a.txt");
        let args = ["cat-file", "blob", blob.as_str()];
        assert!(run(dir.path(), &args, 64, later()).is_some());
        assert_eq!(run(dir.path(), &args, 4, later()), None);
        assert_eq!(run(dir.path(), &args, 64, Instant::now()), None);
        assert_eq!(
            run(&PathBuf::from("/nonexistent"), &args, 64, later()),
            None
        );
    }
}
