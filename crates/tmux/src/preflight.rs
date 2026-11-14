//! Read-only executable preflight; no server connection or product activation.

use std::{cmp::Ordering, io, path::Path, process::Command, time::Duration};

use unicode_properties::{GeneralCategory, GeneralCategoryGroup, UnicodeGeneralCategory};

const CANNOT_RUN: &str =
    "Cannot run tmux -V; tmux 3.4 or newer is required. Check the tmux executable in PATH.";
const UNKNOWN: &str = "Cannot verify tmux version; tmux 3.4 or newer is required. Install a numbered build and check tmux -V.";

/// Verify the executable selected by the connection layer, including its
/// platform-specific fallback locations, without contacting a server.
pub(crate) fn check_executable(binary: &Path) -> io::Result<String> {
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
            "{version} detected; tmux 3.4 or newer is required. Upgrade tmux."
        ))),
        None => Err(io::Error::other(UNKNOWN)),
    }
}

/// Verify the connected server's `#{version}` reply before enabling pane I/O.
/// A newer executable does not imply that an already-running server is supported.
/// This only validates a report; it runs no command and changes no server state.
pub fn check_server_version(version: &str) -> io::Result<()> {
    match supported(&format!("tmux {version}")) {
        Some(true) => Ok(()),
        Some(false) => Err(io::Error::other(format!(
            "tmux server {version} detected; tmux 3.4 or newer is required. Upgrade the server."
        ))),
        None => Err(io::Error::other(
            "Cannot verify tmux server version; tmux 3.4 or newer is required. Use a numbered server build.",
        )),
    }
}

fn supported(report: &str) -> Option<bool> {
    at_least(report, 3, 4)
}

// Python: tmux (?:next-)?(\d+)\.(\d+)(?:[a-zA-Z][\w.-]*|[-+][\w.-]+)?
fn at_least(report: &str, floor_major: u32, floor_minor: u32) -> Option<bool> {
    let version = report.strip_prefix("tmux ")?;
    let version = version.strip_prefix("next-").unwrap_or(version);
    let (major, rest) = number(version, floor_major)?;
    let (minor, suffix) = number(rest.strip_prefix('.')?, floor_minor)?;
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

// Compare with `floor` without imposing a machine-integer limit on the report.
fn number(text: &str, floor: u32) -> Option<(Ordering, &str)> {
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
        value = (value * 10 + digit).min(floor + 1);
        end = index + ch.len_utf8();
    }
    (end > 0).then(|| (value.cmp(&floor), &text[end..]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{env, fs, os::unix::fs::PermissionsExt, time::Instant};

    #[test]
    fn version_floor_accepts_patch_and_numbered_development_builds() {
        for (version, expected) in [
            ("tmux 3.3", Some(false)),
            ("tmux 3.4", Some(true)),
            ("tmux 3.3a", Some(false)),
            ("tmux 3.4a", Some(true)),
            ("tmux 3.8-rc1", Some(true)),
            ("tmux next-3.9", Some(true)),
            ("tmux 4.0+local", Some(true)),
            ("tmux 03.004", Some(true)),
            ("tmux 3.2a", Some(false)),
            ("tmux next-2.99", Some(false)),
            ("tmux 99999999999999999999999.0", Some(true)),
            ("tmux ٣.٤", Some(true)),
            ("tmux 𝟛.𝟚", Some(false)),
            ("tmux 3.4aβ", Some(true)),
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
    fn preflight_clears_the_tmux_context_and_bounds_failures() {
        let directory = tempfile::tempdir().unwrap();
        let binary = directory.path().join("tmux");
        const VALID: &str = r#"#!/bin/sh
[ "$#" -eq 1 ] && [ "$1" = -V ] && [ -z "${TMUX+x}" ] && [ -z "${TMUX_PANE+x}" ] || exit 42
printf ' tmux 3.7c\n'
"#;
        for (mode, script) in [
            ("valid", VALID),
            ("old", "#!/bin/sh\nprintf 'tmux 3.2a\\n'\n"),
            (
                "unknown",
                "#!/bin/sh\nprintf 'fixture-private-output'\nprintf 'fixture-private-stderr' >&2\n",
            ),
            ("failed", "#!/bin/sh\nprintf 'tmux 3.7c\\n'\nexit 1\n"),
            ("broken", "#!/missing-preflight-fixture-interpreter\n"),
            ("timeout", "#!/bin/sh\nexec /bin/sleep 30\n"),
        ] {
            fs::write(&binary, script).unwrap();
            fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
            let started = Instant::now();
            let status = Command::new(env::current_exe().unwrap())
                .args([
                    "--exact",
                    "preflight::tests::preflight_fixture_child",
                    "--nocapture",
                ])
                .env("TMUX_PREFLIGHT_BINARY", &binary)
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
    }

    #[test]
    fn preflight_fixture_child() {
        let Ok(mode) = env::var("TMUX_PREFLIGHT_FIXTURE") else {
            return;
        };
        let result = check_executable(Path::new(&env::var_os("TMUX_PREFLIGHT_BINARY").unwrap()));
        match mode.as_str() {
            "valid" => assert_eq!(result.unwrap(), "tmux 3.7c"),
            "old" => assert_eq!(
                result.unwrap_err().to_string(),
                "tmux 3.2a detected; tmux 3.4 or newer is required. Upgrade tmux."
            ),
            "broken" | "timeout" => assert_eq!(result.unwrap_err().to_string(), CANNOT_RUN),
            "unknown" | "failed" => assert_eq!(result.unwrap_err().to_string(), UNKNOWN),
            _ => panic!("unknown fixture"),
        }
    }
}
