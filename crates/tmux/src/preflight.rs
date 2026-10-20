//! Read-only executable preflight; no server connection or product activation.

use std::{cmp::Ordering, env, io, path::Path, process::Command, time::Duration};

use unicode_properties::{GeneralCategory, GeneralCategoryGroup, UnicodeGeneralCategory};

const MISSING: &str = "tmux not found; tmux 3.3 or newer is required. Install it on this host.";
const CANNOT_RUN: &str =
    "Cannot run tmux -V; tmux 3.3 or newer is required. Check the tmux executable in PATH.";
const UNKNOWN: &str = "Cannot verify tmux version; tmux 3.3 or newer is required. Install a numbered build and check tmux -V.";

/// Find tmux on PATH and verify its numbered version is at least 3.3.
///
/// Runs only `tmux -V`, which contacts no server, with a five-second deadline
/// and bounded stdout/stderr capture. Patch and numbered development suffixes
/// follow the Python preflight. Returns the trimmed version report on success.
/// Failed/malformed output and stderr are never included in diagnostics.
/// Call on an I/O worker; startup wiring and server-version checks are separate.
pub fn check_tmux() -> io::Result<String> {
    let paths = env::var_os("PATH").unwrap_or_else(|| "/bin:/usr/bin".into());
    if paths.is_empty() {
        return Err(io::Error::other(MISSING));
    }
    // Keep PATH entries literal, including tilde. An empty entry within a
    // nonempty PATH means this directory; use ./tmux to avoid a second search.
    let binary = env::split_paths(&paths)
        .map(|directory| {
            if directory.as_os_str().is_empty() {
                Path::new(".").join("tmux")
            } else {
                directory.join("tmux")
            }
        })
        .find(|path| {
            !path.is_dir() && rustix::fs::access(path, rustix::fs::Access::EXEC_OK).is_ok()
        })
        .ok_or_else(|| io::Error::other(MISSING))?;
    let mut command = Command::new(binary);
    command.arg("-V").env_remove("TMUX").env_remove("TMUX_PANE");
    let output = super::discovery::capture(command, Duration::from_secs(5))
        .map_err(|_| io::Error::other(CANNOT_RUN))?;
    let version = std::str::from_utf8(&output.stdout)
        .map_err(|_| io::Error::other(UNKNOWN))?
        .trim_matches(|ch: char| ch.is_whitespace() || matches!(ch, '\u{1c}'..='\u{1f}'));
    if !output.status.success() {
        return Err(io::Error::other(UNKNOWN));
    }
    match supported(version) {
        Some(true) => Ok(version.to_owned()),
        Some(false) => Err(io::Error::other(format!(
            "{version} detected; tmux 3.3 or newer is required. Upgrade tmux."
        ))),
        None => Err(io::Error::other(UNKNOWN)),
    }
}

// Python: tmux (?:next-)?(\d+)\.(\d+)(?:[a-zA-Z][\w.-]*|[-+][\w.-]+)?
fn supported(report: &str) -> Option<bool> {
    let version = report.strip_prefix("tmux ")?;
    let version = version.strip_prefix("next-").unwrap_or(version);
    let (major, rest) = number(version)?;
    let (minor, suffix) = number(rest.strip_prefix('.')?)?;
    if let Some(first) = suffix.chars().next() {
        let rest = &suffix[first.len_utf8()..];
        if !(first.is_ascii_alphabetic() || matches!(first, '-' | '+') && !rest.is_empty())
            || !rest.chars().all(|ch| {
                matches!(ch, '_' | '.' | '-')
                    || matches!(
                        ch.general_category_group(),
                        GeneralCategoryGroup::Letter | GeneralCategoryGroup::Number
                    )
            })
        {
            return None;
        }
    }
    Some(major > Ordering::Equal || major == Ordering::Equal && minor != Ordering::Less)
}

// Compare with three without imposing a machine-integer limit on the report.
fn number(text: &str) -> Option<(Ordering, &str)> {
    let mut value = 0;
    let mut end = 0;
    for (index, ch) in text.char_indices() {
        if ch.general_category() != GeneralCategory::DecimalNumber {
            break;
        }
        // Unicode decimal digits occur in ascending groups of ten. Adjacent
        // groups (for example mathematical digits) are handled by modulo ten.
        let mut start = ch as u32;
        while char::from_u32(start - 1)
            .is_some_and(|previous| previous.general_category() == GeneralCategory::DecimalNumber)
        {
            start -= 1;
        }
        let digit = (ch as u32 - start) % 10;
        value = (value * 10 + digit).min(4);
        end = index + ch.len_utf8();
    }
    (end > 0).then(|| (value.cmp(&3), &text[end..]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt, time::Instant};

    #[test]
    fn version_floor_accepts_patch_and_numbered_development_builds() {
        for (version, expected) in [
            ("tmux 3.3", Some(true)),
            ("tmux 3.3a", Some(true)),
            ("tmux 3.8-rc1", Some(true)),
            ("tmux next-3.9", Some(true)),
            ("tmux 4.0+local", Some(true)),
            ("tmux 03.003", Some(true)),
            ("tmux 3.2a", Some(false)),
            ("tmux next-2.99", Some(false)),
            ("tmux 99999999999999999999999.0", Some(true)),
            ("tmux ٣.٣", Some(true)),
            ("tmux 𝟛.𝟚", Some(false)),
            ("tmux 3.3aβ", Some(true)),
            ("tmux next", None),
            ("tmux 3", None),
            ("tmux 3.3+", None),
            ("tmux 3.3.1", None),
            ("tmux 3.3a+1", None),
            ("tmux 3.3a\u{301}", None),
            ("tmux 3.3 trailing", None),
        ] {
            assert_eq!(supported(version), expected, "{version}");
        }
    }

    #[test]
    fn public_preflight_uses_path_only_and_bounds_failures() {
        let directory = tempfile::tempdir().unwrap();
        let binary = directory.path().join("tmux");
        const VALID: &str = r#"#!/bin/sh
[ "$#" -eq 1 ] && [ "$1" = -V ] && [ -z "${TMUX+x}" ] && [ -z "${TMUX_PANE+x}" ] || exit 42
printf ' tmux 3.7c\n'
"#;
        for (mode, script) in [
            ("valid", Some(VALID)),
            ("nonexec", Some(VALID)),
            ("old", Some("#!/bin/sh\nprintf 'tmux 3.2a\\n'\n")),
            (
                "unknown",
                Some(
                    "#!/bin/sh\nprintf 'fixture-private-output'\nprintf 'fixture-private-stderr' >&2\n",
                ),
            ),
            ("failed", Some("#!/bin/sh\nprintf 'tmux 3.7c\\n'\nexit 1\n")),
            ("broken", Some("#!/missing-preflight-fixture-interpreter\n")),
            ("timeout", Some("#!/bin/sh\nexec /bin/sleep 30\n")),
            ("missing", None),
        ] {
            if let Some(script) = script {
                fs::write(&binary, script).unwrap();
                fs::set_permissions(
                    &binary,
                    fs::Permissions::from_mode(if mode == "nonexec" { 0o600 } else { 0o700 }),
                )
                .unwrap();
            } else {
                fs::remove_file(&binary).unwrap();
            }
            let started = Instant::now();
            let status = Command::new(env::current_exe().unwrap())
                .args([
                    "--exact",
                    "preflight::tests::preflight_fixture_child",
                    "--nocapture",
                ])
                .env("PATH", directory.path())
                .env("TMUX_PREFLIGHT_FIXTURE", mode)
                .env("TMUX", "fixture-context")
                .env("TMUX_PANE", "%fixture")
                .status()
                .unwrap();
            assert!(status.success(), "{mode}");
            if mode == "timeout" {
                assert!(started.elapsed() < Duration::from_secs(15));
            }
        }
        let literal = directory.path().join("~");
        fs::create_dir(&literal).unwrap();
        let binary = literal.join("tmux");
        fs::write(&binary, VALID).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        let status = Command::new(env::current_exe().unwrap())
            .args(["--exact", "preflight::tests::preflight_fixture_child"])
            .current_dir(directory.path())
            .env("PATH", "~")
            .env("HOME", directory.path().join("unused-home"))
            .env("TMUX_PREFLIGHT_FIXTURE", "valid")
            .status()
            .unwrap();
        assert!(status.success());
        let status = Command::new(env::current_exe().unwrap())
            .args(["--exact", "preflight::tests::preflight_fixture_child"])
            .current_dir(&literal)
            .env("PATH", "")
            .env("TMUX_PREFLIGHT_FIXTURE", "missing")
            .status()
            .unwrap();
        assert!(status.success());
    }

    #[test]
    fn preflight_fixture_child() {
        let Ok(mode) = env::var("TMUX_PREFLIGHT_FIXTURE") else {
            return;
        };
        let result = check_tmux();
        match mode.as_str() {
            "valid" => assert_eq!(result.unwrap(), "tmux 3.7c"),
            "old" => assert_eq!(
                result.unwrap_err().to_string(),
                "tmux 3.2a detected; tmux 3.3 or newer is required. Upgrade tmux."
            ),
            "missing" | "nonexec" => assert_eq!(result.unwrap_err().to_string(), MISSING),
            "broken" | "timeout" => assert_eq!(result.unwrap_err().to_string(), CANNOT_RUN),
            "unknown" | "failed" => assert_eq!(result.unwrap_err().to_string(), UNKNOWN),
            _ => panic!("unknown fixture"),
        }
    }
}
