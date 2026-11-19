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

use super::Until;

/// The most output one Git command may write.
pub const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;
/// The longest one Git command may run.
const TIMEOUT: Duration = Duration::from_secs(3);
/// The most rows read from one diff, and lines shown from one file.
pub const ROW_LIMIT: usize = 20_000;

/// The standard output of `git -C dir args…`, when it succeeds before
/// `until` (and within 3 s) and writes at most `limit` bytes. Cancelling
/// `until` stops a running command.
pub fn run(dir: &Path, args: &[&str], limit: usize, until: &Until) -> Option<Vec<u8>> {
    // The first check on macOS reads the file system, so the deadline and a
    // cancel are checked after it, just before Git starts.
    if !runs_git() || until.passed() {
        return None;
    }
    let mut child = command(dir, args).spawn().ok()?;
    let stop = |child: &mut std::process::Child| {
        let _ = child.kill();
        let _ = child.wait();
    };
    let Some(mut stdout) = child.stdout.take() else {
        stop(&mut child);
        return None;
    };
    let reader = thread::Builder::new()
        .name("git output".into())
        .spawn(move || {
            let mut out = Vec::new();
            let read = (&mut stdout).take(limit as u64 + 1).read_to_end(&mut out);
            read.ok().filter(|_| out.len() <= limit).map(|_| out)
        });
    let Ok(reader) = reader else {
        stop(&mut child);
        return None;
    };
    let deadline = until.at().min(Instant::now() + TIMEOUT);
    let over = || Instant::now() >= deadline || until.passed();
    while !reader.is_finished() {
        if over() {
            // The reader ends when the output closes; it is not waited for,
            // in case something Git started still holds the output open.
            stop(&mut child);
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
            Ok(None) if !over() => thread::sleep(Duration::from_millis(2)),
            _ => {
                stop(&mut child);
                return None;
            }
        }
    }
}

/// Whether `git` runs Git. On macOS, Apple's `/usr/bin/git` without the
/// developer tools only offers to install them, so no Git is run then.
fn runs_git() -> bool {
    #[cfg(target_os = "macos")]
    {
        static RUNS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *RUNS.get_or_init(|| {
            let git = found_git(&std::env::var_os("PATH").unwrap_or_default());
            let tools = [
                std::env::var_os("DEVELOPER_DIR").map(std::path::PathBuf::from),
                std::fs::read_link("/var/db/xcode_select_link").ok(),
                Some("/Library/Developer/CommandLineTools".into()),
            ];
            shim_runs(git.as_deref(), &tools)
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

/// The `git` a command started with `path` runs: the first executable file
/// of that name, as the command search finds it.
#[cfg(any(target_os = "macos", test))]
fn found_git(path: &std::ffi::OsStr) -> Option<std::path::PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(path)
        .map(|dir| dir.join("git"))
        .find(|git| {
            std::fs::metadata(git)
                .is_ok_and(|file| file.is_file() && file.permissions().mode() & 0o111 != 0)
        })
}

/// Whether `git`, found at `git`, runs Git: anything but `/usr/bin/git`
/// does, and that one only with developer tools in one of `tools`.
#[cfg(any(target_os = "macos", test))]
fn shim_runs(git: Option<&Path>, tools: &[Option<std::path::PathBuf>]) -> bool {
    git != Some(Path::new("/usr/bin/git"))
        || tools
            .iter()
            .flatten()
            .any(|dir| dir.join("usr/bin/git").is_file())
}

/// `git -C dir args…` as every read runs it: no inherited Git environment,
/// and no system or user configuration or attribute files.
fn command(dir: &Path, args: &[&str]) -> Command {
    let mut command = git_alone();
    command
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_ATTR_NOSYSTEM", "1")
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
        .args(["-c", "core.attributesFile=/dev/null"])
        // A larger file is shown as binary, so Git never holds two of them
        // to compare line by line.
        .args(["-c", "core.bigFileThreshold=16m"])
        .arg("--literal-pathspecs")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    command
}

/// `git`, with no inherited Git environment to choose another repository,
/// index or configuration.
fn git_alone() -> Command {
    let mut command = Command::new("git");
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("GIT_") {
            command.env_remove(name);
        }
    }
    command
}

/// Whether the checkout at `top` has `commit`, and it is a commit rather
/// than, say, a tag.
pub fn has_commit(top: &Path, commit: &str, until: &Until) -> bool {
    run(top, &["cat-file", "-t", commit], 64, until).as_deref() == Some(&b"commit\n"[..])
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

/// What both diff passes are given. `--submodule=short` overrides any
/// `diff.submodule` setting, so Git never reads inside a submodule.
const DIFF: [&str; 6] = [
    "diff",
    "--no-color",
    "--no-ext-diff",
    "--no-textconv",
    "--submodule=short",
    "-M",
];
/// The file list alone: with any patch option Git would write patches too.
const LIST: [&str; 3] = ["--raw", "-z", "--ignore-submodules=none"];

/// `git diff BASE HEAD` of the two commits' trees, limited to `paths`. A
/// diff that touches a submodule is not read.
pub fn diff(
    top: &Path,
    base: &str,
    head: &str,
    paths: Option<&[String]>,
    until: &Until,
) -> Option<Vec<File>> {
    // Attributes, such as which files are binary, come from the new commit,
    // not from whatever the checkout has; a Git without this option answers
    // nothing.
    let attributes = format!("--attr-source={head}");
    let mut tail = vec![base, head, "--"];
    tail.extend(paths.unwrap_or_default().iter().map(String::as_str));
    // The file list, with exact names, then the patch in the same order.
    let raw = run(
        top,
        &[&[&*attributes][..], &DIFF[..], &LIST[..], &tail[..]].concat(),
        OUTPUT_LIMIT,
        until,
    )?;
    let entries = entries(&raw)?;
    // A submodule's patch could run Git inside it.
    if entries.iter().any(|entry| entry.submodule) {
        return None;
    }
    let patch = run(
        top,
        &[&[&*attributes][..], &DIFF[..], &["-U3"][..], &tail[..]].concat(),
        OUTPUT_LIMIT,
        until,
    )?;
    // Git calls a file binary only from its first bytes.
    if patch.contains(&0) {
        return None;
    }
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
        let mut next = || String::from_utf8(fields.next()?.to_vec()).ok();
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
        // Text that is not UTF-8 is not shown at all.
        let text = || String::from_utf8(line[1.min(line.len())..].to_vec()).ok();
        let line_row = |sign, old, new| {
            Some(Row::Line {
                sign,
                old,
                new,
                text: text()?,
            })
        };
        let row = if line.starts_with(b"@@ ") {
            let header = String::from_utf8(line.to_vec()).ok()?;
            (old, new) = starts(&header);
            in_hunk = true;
            Row::Hunk(header)
        } else if !in_hunk {
            if !line.starts_with(b"Binary files ") {
                continue;
            }
            Row::Binary
        } else {
            match line.first() {
                Some(b'+') => {
                    new += 1;
                    line_row('+', None, Some(new - 1))?
                }
                Some(b'-') => {
                    old += 1;
                    line_row('-', Some(old - 1), None)?
                }
                Some(b'\\') => Row::Remark(String::from_utf8(line.to_vec()).ok()?),
                // A context line; an empty one is how `diff.suppressBlankEmpty`
                // writes it.
                first => {
                    old += 1;
                    new += 1;
                    let text = if first.is_some() {
                        text()?
                    } else {
                        String::new()
                    };
                    Row::Line {
                        sign: ' ',
                        old: Some(old - 1),
                        new: Some(new - 1),
                        text,
                    }
                }
            }
        };
        // Only a line that makes a row counts towards the budget.
        if rows.len() >= room {
            return None;
        }
        rows.push(row);
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

/// The text of `path` at `commit`, unless it is missing, binary or not UTF-8.
pub fn file(top: &Path, commit: &str, path: &str, until: &Until) -> Option<String> {
    let blob = run(
        top,
        &["cat-file", "blob", &format!("{commit}:{path}")],
        OUTPUT_LIMIT,
        until,
    )?;
    if blob.contains(&0) {
        return None;
    }
    String::from_utf8(blob).ok()
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::{fs, path::PathBuf};

    /// Run Git for a test fixture, with no user or system configuration.
    pub fn git(dir: &Path, args: &[&str]) -> String {
        let output = git_alone()
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

    fn later() -> Until {
        Until::new(Instant::now() + Duration::from_secs(10))
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
        let files = diff(dir.path(), &first, &second, None, &later()).unwrap();
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
        let files = diff(dir.path(), &first, &second, Some(&paths), &later()).unwrap();
        assert_eq!(
            files.iter().map(|file| &*file.path).collect::<Vec<_>>(),
            ["sp ace.txt"]
        );
        // Paths are literal, never patterns.
        let star = ["*.txt".to_owned()];
        assert_eq!(
            diff(dir.path(), &first, &second, Some(&star), &later()),
            Some(vec![])
        );
    }

    #[test]
    fn a_file_is_read_whole_and_missing_or_binary_files_are_not() {
        let dir = tempfile::tempdir().unwrap();
        let (first, second) = repository(dir.path(), "git@github.com:owner/name.git");
        assert_eq!(
            file(dir.path(), &second, "a.txt", &later()).as_deref(),
            Some("one\nTWO\n\nthree\n")
        );
        assert_eq!(
            file(dir.path(), &first, "old.txt", &later()).as_deref(),
            Some("moved\n")
        );
        assert_eq!(file(dir.path(), &second, "old.txt", &later()), None);
        assert_eq!(file(dir.path(), &second, "bin.dat", &later()), None);
        assert!(has_commit(dir.path(), &first, &later()));
        assert!(!has_commit(dir.path(), &"0".repeat(40), &later()));
        // A tag is not a commit, and a replacement never stands in for one.
        git(dir.path(), &["tag", "-a", "v1", "-m", "tag"]);
        assert!(!has_commit(
            dir.path(),
            &git(dir.path(), &["rev-parse", "v1"]),
            &later()
        ));
        git(dir.path(), &["replace", &first, &second]);
        assert_eq!(
            file(dir.path(), &first, "old.txt", &later()).as_deref(),
            Some("moved\n")
        );
        let missing = "0".repeat(40);
        assert_eq!(diff(dir.path(), &missing, &second, None, &later()), None);
    }

    #[test]
    fn a_file_over_the_big_file_threshold_is_shown_as_binary() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        let big = "x\n".repeat(9 * 1024 * 1024);
        let mut commits = Vec::new();
        for text in [big.clone(), big.replacen('x', "y", 1)] {
            fs::write(dir.path().join("big.txt"), text).unwrap();
            git(dir.path(), &["add", "-A"]);
            git(dir.path(), &["commit", "-qm", "big"]);
            commits.push(git(dir.path(), &["rev-parse", "HEAD"]));
        }
        let files = diff(dir.path(), &commits[0], &commits[1], None, &later()).unwrap();
        assert_eq!(files[0].rows, vec![Row::Binary]);
    }

    #[test]
    fn nothing_runs_inside_a_submodule() {
        let root = tempfile::tempdir().unwrap();
        let (sub, top) = (root.path().join("sub"), root.path().join("top"));
        for dir in [&sub, &top] {
            fs::create_dir(dir).unwrap();
            git(dir, &["init", "-q"]);
        }
        fs::write(sub.join("a.txt"), "1\n").unwrap();
        fs::write(sub.join(".gitattributes"), "* diff=marker\n").unwrap();
        git(&sub, &["add", "-A"]);
        git(&sub, &["commit", "-qm", "one"]);
        let source = sub.to_str().unwrap();
        let add = [
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            source,
            "sub",
        ];
        git(&top, &add);
        git(&top, &["commit", "-qm", "add"]);
        let first = git(&top, &["rev-parse", "HEAD"]);
        fs::write(top.join("sub/a.txt"), "2\n").unwrap();
        git(&top.join("sub"), &["commit", "-qam", "two"]);
        git(&top, &["add", "sub"]);
        git(&top, &["commit", "-qm", "bump"]);
        let second = git(&top, &["rev-parse", "HEAD"]);
        // The checkout asks for submodule diffs, and the submodule names a
        // driver that would leave a mark.
        let marker = root.path().join("marker");
        git(&top, &["config", "diff.submodule", "diff"]);
        let driver = format!("touch {}; cat", marker.display());
        git(
            &top.join("sub"),
            &["config", "diff.marker.textconv", &driver],
        );
        git(
            &top.join("sub"),
            &["config", "diff.marker.command", &driver],
        );
        let list = [
            &DIFF[..],
            &LIST[..],
            &[first.as_str(), second.as_str(), "--"][..],
        ]
        .concat();
        let raw = run(&top, &list, OUTPUT_LIMIT, &later()).unwrap();
        // Raw records only: a patch, the submodule's own included, has lines.
        assert!(!raw.is_empty() && !raw.contains(&b'\n'));
        assert_eq!(diff(&top, &first, &second, None, &later()), None);
        assert!(!marker.exists());
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
            diff(dir.path(), &commits[0], &commits[1], None, &later()),
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
        // A file with no rows after a full budget still fits.
        let raw = b":100644 100644 0000000 1111111 M\0x\0:100644 100755 2222222 2222222 M\0y\0";
        let patch = format!(
            "{}diff --git a/y b/y\nold mode 100644\nnew mode 100755\n",
            patch(ROW_LIMIT - 1)
        );
        assert!(files(entries(raw).unwrap(), patch.as_bytes()).is_some());
    }

    #[test]
    fn every_read_leaves_out_outside_configuration_and_the_network() {
        use std::ffi::OsStr;
        let command = command(Path::new("/"), &["status"]);
        let envs: std::collections::HashMap<_, _> = command.get_envs().collect();
        for (name, value) in [
            ("GIT_CONFIG_GLOBAL", "/dev/null"),
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_ATTR_NOSYSTEM", "1"),
            ("GIT_NO_LAZY_FETCH", "1"),
            ("GIT_ALLOW_PROTOCOL", ""),
            ("GIT_NO_REPLACE_OBJECTS", "1"),
            ("GIT_OPTIONAL_LOCKS", "0"),
        ] {
            assert_eq!(
                envs.get(OsStr::new(name)),
                Some(&Some(OsStr::new(value))),
                "{name}"
            );
        }
        let args: Vec<_> = command.get_args().collect();
        for flag in [
            "protocol.allow=never",
            "core.attributesFile=/dev/null",
            "core.bigFileThreshold=16m",
            "--literal-pathspecs",
        ] {
            assert!(args.contains(&OsStr::new(flag)), "{flag}");
        }
    }

    #[test]
    fn text_that_is_not_utf8_is_not_read() {
        let dir = tempfile::tempdir().unwrap();
        let (_, second) = repository(dir.path(), "git@github.com:owner/name.git");
        fs::write(dir.path().join("latin.txt"), b"caf\xe9\n").unwrap();
        git(dir.path(), &["add", "-A"]);
        git(dir.path(), &["commit", "-qm", "latin"]);
        let third = git(dir.path(), &["rev-parse", "HEAD"]);
        assert_eq!(file(dir.path(), &third, "latin.txt", &later()), None);
        assert_eq!(diff(dir.path(), &second, &third, None, &later()), None);
    }

    #[test]
    fn a_nul_byte_anywhere_is_not_read() {
        let dir = tempfile::tempdir().unwrap();
        let (_, second) = repository(dir.path(), "git@github.com:owner/name.git");
        let late = format!("{}\0\n", "a\n".repeat(5000));
        fs::write(dir.path().join("late.txt"), late).unwrap();
        git(dir.path(), &["add", "-A"]);
        git(dir.path(), &["commit", "-qm", "late"]);
        let third = git(dir.path(), &["rev-parse", "HEAD"]);
        assert_eq!(file(dir.path(), &third, "late.txt", &later()), None);
        assert_eq!(diff(dir.path(), &second, &third, None, &later()), None);
    }

    #[test]
    fn apples_git_stub_runs_git_only_with_developer_tools() {
        let tools = tempfile::tempdir().unwrap();
        let stub = Some(Path::new("/usr/bin/git"));
        assert!(!shim_runs(stub, &[None, Some("/nonexistent".into())]));
        assert!(shim_runs(Some(Path::new("/opt/homebrew/bin/git")), &[]));
        fs::create_dir_all(tools.path().join("usr/bin")).unwrap();
        fs::write(tools.path().join("usr/bin/git"), "").unwrap();
        assert!(shim_runs(stub, &[Some(tools.path().to_owned())]));
        // A git that cannot run is passed over, as the command search does.
        use std::os::unix::fs::PermissionsExt;
        let (first, second) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        fs::write(first.path().join("git"), "").unwrap();
        let stand_in = second.path().join("git");
        fs::write(&stand_in, "").unwrap();
        fs::set_permissions(&stand_in, fs::Permissions::from_mode(0o755)).unwrap();
        let path = std::env::join_paths([first.path(), second.path()]).unwrap();
        assert_eq!(found_git(&path), Some(stand_in));
    }

    #[test]
    fn output_and_time_are_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let (_, second) = repository(dir.path(), "git@github.com:owner/name.git");
        let blob = format!("{second}:a.txt");
        let args = ["cat-file", "blob", blob.as_str()];
        assert!(run(dir.path(), &args, 64, &later()).is_some());
        assert_eq!(run(dir.path(), &args, 4, &later()), None);
        assert_eq!(
            run(dir.path(), &args, 64, &Until::new(Instant::now())),
            None
        );
        // A command still running at its deadline is stopped, and the read
        // returns soon after.
        let started = Instant::now();
        let slow = ["-c", "alias.slow=!sleep 3", "slow"];
        let soon = started + Duration::from_millis(300);
        assert_eq!(run(dir.path(), &slow, 64, &Until::new(soon)), None);
        assert!(started.elapsed() < Duration::from_secs(2));
        // Cancelling stops a command that is running.
        let until = later();
        let cancel = until.clone();
        let started = Instant::now();
        let stopper = thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            cancel.cancel();
        });
        assert_eq!(run(dir.path(), &slow, 64, &until), None);
        assert!(started.elapsed() < Duration::from_secs(2));
        stopper.join().unwrap();
        assert_eq!(
            run(&PathBuf::from("/nonexistent"), &args, 64, &later()),
            None
        );
    }
}
